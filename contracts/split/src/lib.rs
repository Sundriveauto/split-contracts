//! StellarSplit — on-chain invoice & payment splitting contract.
//!
//! Allows a creator to define an invoice with multiple recipients and amounts.
//! Payers contribute funds; once fully funded the contract auto-routes USDC to
//! each recipient. If the deadline passes unfunded, payers are refunded.

#![no_std]

mod events;
mod types;

#[cfg(test)]
mod test;

use soroban_sdk::{contract, contractimpl, symbol_short, token, Address, BytesN, Env, Symbol, Vec};
use types::{AuditEntry, CompletionProof, Invoice, InvoiceStatus, Payment, SubscriptionParams};

// ---------------------------------------------------------------------------
// Storage helpers
// ---------------------------------------------------------------------------

fn counter_key() -> Symbol {
    symbol_short!("counter")
}

fn invoice_key(id: u64) -> (Symbol, u64) {
    (symbol_short!("inv"), id)
}

fn load_invoice(env: &Env, id: u64) -> Invoice {
    env.storage()
        .persistent()
        .get(&invoice_key(id))
        .expect("invoice not found")
}

fn save_invoice(env: &Env, id: u64, invoice: &Invoice) {
    env.storage().persistent().set(&invoice_key(id), invoice);
}

fn audit_log_key(id: u64) -> (Symbol, u64) {
    (symbol_short!("log"), id)
}

fn subscription_params_key(id: u64) -> (Symbol, u64) {
    (symbol_short!("sub"), id)
}

fn installment_key(invoice_id: u64, payer: &Address) -> (Symbol, u64, Address) {
    (symbol_short!("install"), invoice_id, payer.clone())
}

fn whitelist_key() -> Symbol {
    symbol_short!("wl")
}

fn admin_key() -> Symbol {
    symbol_short!("admin")
}

fn append_audit_entry(env: &Env, id: u64, action: Symbol, actor: &Address) {
    let entry = AuditEntry {
        action,
        actor: actor.clone(),
        timestamp: env.ledger().timestamp(),
    };
    let mut log: Vec<AuditEntry> = env
        .storage()
        .persistent()
        .get(&audit_log_key(id))
        .unwrap_or_else(|| Vec::new(env));
    log.push_back(entry);
    env.storage().persistent().set(&audit_log_key(id), &log);
}

pub fn get_audit_log(env: &Env, id: u64) -> Vec<AuditEntry> {
    env.storage()
        .persistent()
        .get(&audit_log_key(id))
        .unwrap_or_else(|| Vec::new(env))
}

// ---------------------------------------------------------------------------
// Contract
// ---------------------------------------------------------------------------

#[contract]
pub struct SplitContract;

#[contractimpl]
impl SplitContract {
    // -----------------------------------------------------------------------
    // Admin / whitelist
    // -----------------------------------------------------------------------

    /// Initialise the contract admin. Can only be called once.
    pub fn init_admin(env: Env, admin: Address) {
        assert!(
            !env.storage().persistent().has(&admin_key()),
            "admin already set"
        );
        env.storage().persistent().set(&admin_key(), &admin);
    }

    /// Add a token to the whitelist. Requires admin auth.
    pub fn whitelist_token(env: Env, token: Address) {
        let admin: Address = env
            .storage()
            .persistent()
            .get(&admin_key())
            .expect("admin not set");
        admin.require_auth();

        let mut wl: Vec<Address> = env
            .storage()
            .persistent()
            .get(&whitelist_key())
            .unwrap_or_else(|| Vec::new(&env));
        if !wl.contains(&token) {
            wl.push_back(token);
        }
        env.storage().persistent().set(&whitelist_key(), &wl);
    }

    /// Remove a token from the whitelist. Requires admin auth.
    pub fn remove_token(env: Env, token: Address) {
        let admin: Address = env
            .storage()
            .persistent()
            .get(&admin_key())
            .expect("admin not set");
        admin.require_auth();

        let wl: Vec<Address> = env
            .storage()
            .persistent()
            .get(&whitelist_key())
            .unwrap_or_else(|| Vec::new(&env));
        let mut new_wl: Vec<Address> = Vec::new(&env);
        for t in wl.iter() {
            if t != token {
                new_wl.push_back(t);
            }
        }
        env.storage().persistent().set(&whitelist_key(), &new_wl);
    }

    // -----------------------------------------------------------------------
    // Invoice lifecycle
    // -----------------------------------------------------------------------

    /// Create a new invoice.
    pub fn create_invoice(
        env: Env,
        creator: Address,
        recipients: Vec<Address>,
        amounts: Vec<i128>,
        token: Address,
        deadline: u64,
    ) -> u64 {
        Self::create_invoice_recurring(env, creator, recipients, amounts, token, deadline, false)
    }

    /// Create a new invoice with optional recurring flag.
    pub fn create_invoice_recurring(
        env: Env,
        creator: Address,
        recipients: Vec<Address>,
        amounts: Vec<i128>,
        token: Address,
        deadline: u64,
        recurring: bool,
    ) -> u64 {
        creator.require_auth();

        assert!(
            recipients.len() == amounts.len(),
            "recipients and amounts length mismatch"
        );
        assert!(!recipients.is_empty(), "must have at least one recipient");

        let now = env.ledger().timestamp();
        assert!(deadline > now, "deadline must be in the future");

        for amt in amounts.iter() {
            assert!(amt > 0, "amounts must be positive");
        }

        // Enforce token whitelist if one is configured.
        if env.storage().persistent().has(&whitelist_key()) {
            let wl: Vec<Address> = env
                .storage()
                .persistent()
                .get(&whitelist_key())
                .unwrap_or_else(|| Vec::new(&env));
            assert!(wl.contains(&token), "token not whitelisted");
        }

        let id: u64 = env
            .storage()
            .persistent()
            .get(&counter_key())
            .unwrap_or(0u64)
            + 1;
        env.storage().persistent().set(&counter_key(), &id);

        let total: i128 = amounts.iter().sum();
        let original_duration = deadline - now;

        let invoice = Invoice {
            creator: creator.clone(),
            recipients,
            amounts,
            token,
            deadline,
            funded: 0,
            status: InvoiceStatus::Pending,
            payments: Vec::new(&env),
            recurring,
            original_duration,
        };

        save_invoice(&env, id, &invoice);
        events::invoice_created(&env, id, &creator, total);

        id
    }

    /// Create a subscription chain of invoices for recurring monthly billing.
    pub fn create_subscription(
        env: Env,
        creator: Address,
        recipients: Vec<Address>,
        amounts: Vec<i128>,
        token: Address,
        months: u32,
    ) -> u64 {
        creator.require_auth();

        assert!(
            recipients.len() == amounts.len(),
            "recipients and amounts length mismatch"
        );
        assert!(!recipients.is_empty(), "must have at least one recipient");
        assert!(months > 0 && months <= 12, "months must be between 1 and 12");

        for amt in amounts.iter() {
            assert!(amt > 0, "amounts must be positive");
        }

        let deadline = env.ledger().timestamp() + 30 * 24 * 60 * 60;
        let id = Self::create_invoice(
            env.clone(),
            creator.clone(),
            recipients.clone(),
            amounts.clone(),
            token.clone(),
            deadline,
        );

        if months > 1 {
            let params = SubscriptionParams {
                creator: creator.clone(),
                recipients: recipients.clone(),
                amounts: amounts.clone(),
                token: token.clone(),
            };
            env.storage()
                .persistent()
                .set(&subscription_params_key(id), &params);
        }

        id
    }

    /// Pay toward an invoice.
    pub fn pay(env: Env, payer: Address, invoice_id: u64, amount: i128) {
        payer.require_auth();
        Self::_pay(&env, &payer, invoice_id, amount);
    }

    /// Pay toward multiple invoices in a single call with one auth check.
    ///
    /// All-or-nothing: any invalid payment reverts the entire call.
    pub fn batch_pay(env: Env, payer: Address, payments: Vec<(u64, i128)>) {
        payer.require_auth();
        for (invoice_id, amount) in payments.iter() {
            Self::_pay(&env, &payer, invoice_id, amount);
        }
    }

    /// Release funds to all recipients once the invoice is fully funded.
    pub fn release(env: Env, invoice_id: u64) {
        let caller = env.current_contract_address();
        let mut invoice = load_invoice(&env, invoice_id);

        assert!(
            invoice.status == InvoiceStatus::Pending,
            "invoice is not pending"
        );

        let total: i128 = invoice.amounts.iter().sum();
        assert!(invoice.funded >= total, "invoice not fully funded");

        Self::_release(&env, invoice_id, &mut invoice, &caller);
    }

    /// Refund all payers if the deadline has passed and the invoice is not fully funded.
    pub fn refund(env: Env, invoice_id: u64) {
        let mut invoice = load_invoice(&env, invoice_id);

        assert!(
            invoice.status == InvoiceStatus::Pending,
            "invoice is not pending"
        );
        assert!(
            env.ledger().timestamp() > invoice.deadline,
            "deadline has not passed"
        );

        let token_client = token::Client::new(&env, &invoice.token);

        for payment in invoice.payments.iter() {
            token_client.transfer(
                &env.current_contract_address(),
                &payment.payer,
                &payment.amount,
            );
        }

        invoice.status = InvoiceStatus::Refunded;
        save_invoice(&env, invoice_id, &invoice);
        let actor = env.current_contract_address();
        append_audit_entry(&env, invoice_id, symbol_short!("refund"), &actor);
        events::invoice_refunded(&env, invoice_id);
    }

    /// Cancel an invoice before any payments are made.
    pub fn cancel_invoice(env: Env, caller: Address, invoice_id: u64) {
        caller.require_auth();

        let mut invoice = load_invoice(&env, invoice_id);

        assert!(
            invoice.status == InvoiceStatus::Pending,
            "invoice is not pending"
        );
        assert!(invoice.creator == caller, "only creator can cancel");
        assert!(invoice.funded == 0, "cannot cancel invoice with payments");

        invoice.status = InvoiceStatus::Cancelled;
        save_invoice(&env, invoice_id, &invoice);
        append_audit_entry(&env, invoice_id, symbol_short!("cancel"), &caller);
    }

    /// Extend the deadline for an invoice.
    pub fn extend_deadline(env: Env, caller: Address, invoice_id: u64, new_deadline: u64) {
        caller.require_auth();

        let mut invoice = load_invoice(&env, invoice_id);

        assert!(
            invoice.status == InvoiceStatus::Pending,
            "invoice is not pending"
        );
        assert!(
            invoice.creator == caller,
            "only creator can extend deadline"
        );
        assert!(
            new_deadline > env.ledger().timestamp(),
            "new deadline must be in the future"
        );

        invoice.deadline = new_deadline;
        save_invoice(&env, invoice_id, &invoice);
        append_audit_entry(&env, invoice_id, symbol_short!("extend"), &caller);
    }

    // -----------------------------------------------------------------------
    // Installment plans
    // -----------------------------------------------------------------------

    /// Register a payment schedule for a payer on an invoice.
    ///
    /// The plan is informational — pay() still works normally.
    /// Calling again overwrites the previous plan.
    pub fn register_installment_plan(
        env: Env,
        payer: Address,
        invoice_id: u64,
        schedule: Vec<(u64, i128)>,
    ) {
        payer.require_auth();
        // Verify invoice exists.
        load_invoice(&env, invoice_id);
        env.storage()
            .persistent()
            .set(&installment_key(invoice_id, &payer), &schedule);
    }

    /// Retrieve the installment plan for a payer on an invoice.
    pub fn get_installment_plan(
        env: Env,
        payer: Address,
        invoice_id: u64,
    ) -> Vec<(u64, i128)> {
        env.storage()
            .persistent()
            .get(&installment_key(invoice_id, &payer))
            .unwrap_or_else(|| Vec::new(&env))
    }

    // -----------------------------------------------------------------------
    // Read-only views
    // -----------------------------------------------------------------------

    pub fn get_invoice(env: Env, invoice_id: u64) -> Invoice {
        load_invoice(&env, invoice_id)
    }

    pub fn get_audit_log(env: Env, invoice_id: u64) -> Vec<AuditEntry> {
        get_audit_log(&env, invoice_id)
    }

    pub fn get_completion_proof(env: Env, invoice_id: u64) -> CompletionProof {
        let invoice = load_invoice(&env, invoice_id);

        assert!(
            invoice.status == InvoiceStatus::Released || invoice.status == InvoiceStatus::Refunded,
            "invoice not finalized"
        );

        let mut bytes: Vec<u8> = Vec::new(&env);
        bytes.extend_from_slice(&invoice.creator.to_bytes());
        bytes.push(invoice.recipients.len() as u8);
        for r in invoice.recipients.iter() {
            bytes.extend_from_slice(&r.to_bytes());
        }
        bytes.push((invoice.amounts.len() & 0xFF) as u8);
        bytes.push(((invoice.amounts.len() >> 8) & 0xFF) as u8);
        for a in invoice.amounts.iter() {
            bytes.extend_from_slice(&a.to_le_bytes());
        }
        bytes.extend_from_slice(&invoice.token.to_bytes());
        bytes.extend_from_slice(&invoice.deadline.to_le_bytes());
        bytes.extend_from_slice(&invoice.funded.to_le_bytes());
        let s_byte = match invoice.status {
            InvoiceStatus::Pending => 0u8,
            InvoiceStatus::Released => 1u8,
            InvoiceStatus::Refunded => 2u8,
            InvoiceStatus::Cancelled => 3u8,
        };
        bytes.push(s_byte);

        let hash = env.crypto().sha256(&bytes).to_bytes();

        CompletionProof {
            id: invoice_id,
            status: invoice.status,
            funded: invoice.funded,
            timestamp: env.ledger().timestamp(),
            hash,
        }
    }

    // -----------------------------------------------------------------------
    // Internal helpers
    // -----------------------------------------------------------------------

    fn _pay(env: &Env, payer: &Address, invoice_id: u64, amount: i128) {
        let mut invoice = load_invoice(env, invoice_id);

        assert!(
            invoice.status == InvoiceStatus::Pending,
            "invoice is not pending"
        );
        assert!(
            env.ledger().timestamp() <= invoice.deadline,
            "invoice deadline has passed"
        );
        assert!(amount > 0, "payment amount must be positive");

        let total: i128 = invoice.amounts.iter().sum();
        let remaining = total - invoice.funded;
        assert!(amount <= remaining, "payment exceeds remaining balance");

        let token_client = token::Client::new(env, &invoice.token);
        token_client.transfer(payer, &env.current_contract_address(), &amount);

        invoice.payments.push_back(Payment {
            payer: payer.clone(),
            amount,
        });
        invoice.funded += amount;

        append_audit_entry(env, invoice_id, symbol_short!("pay"), payer);
        events::payment_received(env, invoice_id, payer, amount);

        if invoice.funded >= total {
            Self::_release(env, invoice_id, &mut invoice, payer);
        } else {
            save_invoice(env, invoice_id, &invoice);
        }
    }

    fn _release(env: &Env, invoice_id: u64, invoice: &mut Invoice, actor: &Address) {
        let token_client = token::Client::new(env, &invoice.token);

        for (recipient, amount) in invoice.recipients.iter().zip(invoice.amounts.iter()) {
            token_client.transfer(&env.current_contract_address(), &recipient, &amount);
        }

        invoice.status = InvoiceStatus::Released;
        save_invoice(env, invoice_id, invoice);
        append_audit_entry(env, invoice_id, symbol_short!("release"), actor);
        events::invoice_released(env, invoice_id, &invoice.recipients);

        // Auto-create next invoice for recurring invoices.
        if invoice.recurring {
            let next_deadline = env.ledger().timestamp() + invoice.original_duration;
            let id: u64 = env
                .storage()
                .persistent()
                .get(&counter_key())
                .unwrap_or(0u64)
                + 1;
            env.storage().persistent().set(&counter_key(), &id);

            let next_invoice = Invoice {
                creator: invoice.creator.clone(),
                recipients: invoice.recipients.clone(),
                amounts: invoice.amounts.clone(),
                token: invoice.token.clone(),
                deadline: next_deadline,
                funded: 0,
                status: InvoiceStatus::Pending,
                payments: Vec::new(env),
                recurring: true,
                original_duration: invoice.original_duration,
            };
            save_invoice(env, id, &next_invoice);
            let total: i128 = invoice.amounts.iter().sum();
            events::invoice_created(env, id, &invoice.creator, total);
        }

        // Check for legacy subscription params.
        if let Some(params) = env
            .storage()
            .persistent()
            .get::<_, SubscriptionParams>(&subscription_params_key(invoice_id))
        {
            let next_deadline = env.ledger().timestamp() + 30 * 24 * 60 * 60;
            let id: u64 = env
                .storage()
                .persistent()
                .get(&counter_key())
                .unwrap_or(0u64)
                + 1;
            env.storage().persistent().set(&counter_key(), &id);

            let next_invoice = Invoice {
                creator: params.creator.clone(),
                recipients: params.recipients.clone(),
                amounts: params.amounts.clone(),
                token: params.token.clone(),
                deadline: next_deadline,
                funded: 0,
                status: InvoiceStatus::Pending,
                payments: Vec::new(env),
                recurring: false,
                original_duration: 30 * 24 * 60 * 60,
            };
            save_invoice(env, id, &next_invoice);
            env.storage()
                .persistent()
                .remove(&subscription_params_key(invoice_id));
        }
    }
}
