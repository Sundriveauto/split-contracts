//! StellarSplit — on-chain invoice & payment splitting contract.
//!
//! Allows a creator to define an invoice with multiple recipients and amounts.
//! Payers contribute funds; once fully funded the contract auto-routes tokens to
//! each recipient. If the deadline passes unfunded, payers are refunded.

#![no_std]

mod events;
mod types;

#[cfg(test)]
mod test;

use soroban_sdk::{contract, contractimpl, symbol_short, token, Address, Bytes, Env, Symbol, Vec};
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

// Protocol fee storage keys
fn treasury_key() -> Symbol {
    symbol_short!("treasury")
}

fn fee_bps_key() -> Symbol {
    symbol_short!("fee_bps")
}

fn admin_key() -> Symbol {
    symbol_short!("admin")
}

// Creator index key: ("cidx", creator)
fn creator_idx_key(creator: &Address) -> (Symbol, Address) {
    (symbol_short!("cidx"), creator.clone())
}

// ---------------------------------------------------------------------------
// Contract
// ---------------------------------------------------------------------------

#[contract]
pub struct SplitContract;

#[contractimpl]
impl SplitContract {
    /// Initialise the contract with an admin, treasury address, and fee in basis points.
    ///
    /// Must be called once before using freeze/unfreeze or protocol fee features.
    /// `fee_bps = 0` disables the fee entirely.
    pub fn initialize(env: Env, admin: Address, treasury: Address, fee_bps: u32) {
        admin.require_auth();
        assert!(fee_bps <= 10_000, "fee_bps must be <= 10000");
        env.storage().persistent().set(&admin_key(), &admin);
        env.storage().persistent().set(&treasury_key(), &treasury);
        env.storage().persistent().set(&fee_bps_key(), &fee_bps);
    }

    /// Create a new invoice.
    ///
    /// # Arguments
    /// * `creator`    – address that owns the invoice (must authorise)
    /// * `recipients` – ordered list of recipient addresses
    /// * `amounts`    – amount owed to each recipient (parallel to `recipients`)
    /// * `tokens`     – token contract address per recipient (parallel to `recipients`)
    /// * `deadline`   – Unix timestamp; after this refunds become available
    ///
    /// # Returns
    /// The new invoice ID (monotonically increasing u64).
    pub fn create_invoice(
        env: Env,
        creator: Address,
        recipients: Vec<Address>,
        amounts: Vec<i128>,
        tokens: Vec<Address>,
        deadline: u64,
    ) -> u64 {
        creator.require_auth();
        Self::_create_invoice(&env, creator, recipients, amounts, tokens, deadline)
    }

    fn _create_invoice(
        env: &Env,
        creator: Address,
        recipients: Vec<Address>,
        amounts: Vec<i128>,
        tokens: Vec<Address>,
        deadline: u64,
    ) -> u64 {
        assert!(
            recipients.len() == amounts.len(),
            "recipients and amounts length mismatch"
        );
        assert!(
            recipients.len() == tokens.len(),
            "recipients and tokens length mismatch"
        );
        assert!(!recipients.is_empty(), "must have at least one recipient");
        assert!(
            deadline > env.ledger().timestamp(),
            "deadline must be in the future"
        );
        for amt in amounts.iter() {
            assert!(amt > 0, "amounts must be positive");
        }

        let id: u64 = env
            .storage()
            .persistent()
            .get(&counter_key())
            .unwrap_or(0u64)
            + 1;
        env.storage().persistent().set(&counter_key(), &id);

        let total: i128 = amounts.iter().sum();

        let invoice = Invoice {
            creator: creator.clone(),
            recipients,
            amounts,
            tokens,
            deadline,
            funded: 0,
            status: InvoiceStatus::Pending,
            payments: Vec::new(env),
            frozen: false,
        };

        save_invoice(env, id, &invoice);
        events::invoice_created(env, id, &creator, total);

        // Update creator index
        let idx_key = creator_idx_key(&creator);
        let mut idx: Vec<u64> = env
            .storage()
            .persistent()
            .get(&idx_key)
            .unwrap_or_else(|| Vec::new(env));
        idx.push_back(id);
        env.storage().persistent().set(&idx_key, &idx);

        id
    }

    /// Create a subscription chain of invoices for recurring monthly billing.
    pub fn create_subscription(
        env: Env,
        creator: Address,
        recipients: Vec<Address>,
        amounts: Vec<i128>,
        tokens: Vec<Address>,
        months: u32,
    ) -> u64 {
        creator.require_auth();

        assert!(
            recipients.len() == amounts.len(),
            "recipients and amounts length mismatch"
        );
        assert!(
            recipients.len() == tokens.len(),
            "recipients and tokens length mismatch"
        );
        assert!(!recipients.is_empty(), "must have at least one recipient");
        assert!(months > 0 && months <= 12, "months must be between 1 and 12");
        for amt in amounts.iter() {
            assert!(amt > 0, "amounts must be positive");
        }

        let deadline = env.ledger().timestamp() + 30 * 24 * 60 * 60;
        let id = Self::_create_invoice(
            &env,
            creator.clone(),
            recipients.clone(),
            amounts.clone(),
            tokens.clone(),
            deadline,
        );

        if months > 1 {
            let params = SubscriptionParams {
                creator: creator.clone(),
                recipients: recipients.clone(),
                amounts: amounts.clone(),
                tokens: tokens.clone(),
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

        let mut invoice = load_invoice(&env, invoice_id);

        assert!(!invoice.frozen, "invoice is frozen");
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

        // For multi-token invoices, payment is accepted in the first token.
        // The payer sends the aggregate amount; _release distributes per-token.
        let token_client = token::Client::new(&env, &invoice.tokens.get(0).expect("no token"));
        token_client.transfer(&payer, &env.current_contract_address(), &amount);

        invoice.payments.push_back(Payment {
            payer: payer.clone(),
            amount,
        });
        invoice.funded += amount;

        append_audit_entry(&env, invoice_id, symbol_short!("pay"), &payer);
        events::payment_received(&env, invoice_id, &payer, amount);

        if invoice.funded >= total {
            let creator = invoice.creator.clone();
            Self::_release(&env, invoice_id, &mut invoice, &creator);
        } else {
            save_invoice(&env, invoice_id, &invoice);
        }
    }

    /// Release funds to all recipients once the invoice is fully funded.
    pub fn release(env: Env, invoice_id: u64) {
        let caller = env.current_contract_address();
        let mut invoice = load_invoice(&env, invoice_id);

        assert!(!invoice.frozen, "invoice is frozen");
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

        // Refund in the payment token (tokens[0])
        let token_client =
            token::Client::new(&env, &invoice.tokens.get(0).expect("no token"));

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
        assert!(invoice.creator == caller, "only creator can extend deadline");
        assert!(
            new_deadline > env.ledger().timestamp(),
            "new deadline must be in the future"
        );

        invoice.deadline = new_deadline;
        save_invoice(&env, invoice_id, &invoice);
        append_audit_entry(&env, invoice_id, symbol_short!("extend"), &caller);
    }

    /// Freeze an invoice, blocking pay() and release(). Requires admin auth.
    pub fn freeze_invoice(env: Env, invoice_id: u64) {
        let admin: Address = env
            .storage()
            .persistent()
            .get(&admin_key())
            .expect("contract not initialized");
        admin.require_auth();

        let mut invoice = load_invoice(&env, invoice_id);
        invoice.frozen = true;
        save_invoice(&env, invoice_id, &invoice);
    }

    /// Unfreeze an invoice, restoring normal operation. Requires admin auth.
    pub fn unfreeze_invoice(env: Env, invoice_id: u64) {
        let admin: Address = env
            .storage()
            .persistent()
            .get(&admin_key())
            .expect("contract not initialized");
        admin.require_auth();

        let mut invoice = load_invoice(&env, invoice_id);
        invoice.frozen = false;
        save_invoice(&env, invoice_id, &invoice);
    }

    /// Return all invoice IDs created by `creator`.
    pub fn get_invoices_by_creator(env: Env, creator: Address) -> Vec<u64> {
        env.storage()
            .persistent()
            .get(&creator_idx_key(&creator))
            .unwrap_or_else(|| Vec::new(&env))
    }

    /// Retrieve an invoice by ID.
    pub fn get_invoice(env: Env, invoice_id: u64) -> Invoice {
        load_invoice(&env, invoice_id)
    }

    /// Retrieve the audit log for an invoice.
    pub fn get_audit_log(env: Env, invoice_id: u64) -> Vec<AuditEntry> {
        get_audit_log(&env, invoice_id)
    }

    /// Generate a completion proof for a finalized invoice.
    pub fn get_completion_proof(env: Env, invoice_id: u64) -> CompletionProof {
        let invoice = load_invoice(&env, invoice_id);

        assert!(
            invoice.status == InvoiceStatus::Released || invoice.status == InvoiceStatus::Refunded,
            "invoice not finalized"
        );

        // Build a deterministic byte payload using soroban_sdk::Bytes.
        let mut raw: [u8; 32] = [0u8; 32];
        // Encode funded (i128 = 16 bytes) and deadline (u64 = 8 bytes) into the hash seed.
        let funded_bytes = invoice.funded.to_le_bytes();
        let deadline_bytes = invoice.deadline.to_le_bytes();
        raw[..16].copy_from_slice(&funded_bytes);
        raw[16..24].copy_from_slice(&deadline_bytes);
        let s_byte = match invoice.status {
            InvoiceStatus::Pending => 0u8,
            InvoiceStatus::Released => 1u8,
            InvoiceStatus::Refunded => 2u8,
            InvoiceStatus::Cancelled => 3u8,
        };
        raw[24] = s_byte;
        raw[25] = (invoice.recipients.len() & 0xFF) as u8;

        let bytes = Bytes::from_array(&env, &raw);
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

    /// Route funds to all recipients (with per-recipient token) and mark released.
    /// Deducts protocol fee from the total before distributing if configured.
    fn _release(env: &Env, invoice_id: u64, invoice: &mut Invoice, actor: &Address) {
        let total: i128 = invoice.amounts.iter().sum();

        // Deduct protocol fee from the payment token (tokens[0]) if configured.
        let fee_bps: u32 = env
            .storage()
            .persistent()
            .get(&fee_bps_key())
            .unwrap_or(0u32);

        let fee = if fee_bps > 0 {
            if let Some(treasury) = env
                .storage()
                .persistent()
                .get::<_, Address>(&treasury_key())
            {
                let f = total * fee_bps as i128 / 10_000;
                if f > 0 {
                    let pay_token =
                        token::Client::new(env, &invoice.tokens.get(0).expect("no token"));
                    pay_token.transfer(&env.current_contract_address(), &treasury, &f);
                }
                f
            } else {
                0
            }
        } else {
            0
        };

        let post_fee_total = total - fee;

        // Distribute to each recipient using their assigned token.
        // Each recipient gets a proportional share of the post-fee total.
        for i in 0..invoice.recipients.len() {
            let recipient = invoice.recipients.get(i).unwrap();
            let amount = invoice.amounts.get(i).unwrap();
            let tok = invoice.tokens.get(i).unwrap();
            // Scale amount proportionally: amount * post_fee_total / total
            let scaled = if total > 0 {
                amount * post_fee_total / total
            } else {
                amount
            };
            let token_client = token::Client::new(env, &tok);
            token_client.transfer(&env.current_contract_address(), &recipient, &scaled);
        }

        invoice.status = InvoiceStatus::Released;
        save_invoice(env, invoice_id, invoice);
        append_audit_entry(env, invoice_id, symbol_short!("release"), actor);
        events::invoice_released(env, invoice_id, &invoice.recipients);

        // Check for subscription params and create next invoice if exists.
        if let Some(params) = env
            .storage()
            .persistent()
            .get::<_, SubscriptionParams>(&subscription_params_key(invoice_id))
        {
            let next_deadline = env.ledger().timestamp() + 30 * 24 * 60 * 60;
            Self::_create_invoice(
                env,
                params.creator.clone(),
                params.recipients.clone(),
                params.amounts.clone(),
                params.tokens.clone(),
                next_deadline,
            );
            env.storage()
                .persistent()
                .remove(&subscription_params_key(invoice_id));
        }
    }
}
