use soroban_sdk::{contracttype, Address, BytesN, Env, Symbol, Vec};

#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub enum InvoiceStatus {
    Pending,
    Released,
    Refunded,
    Cancelled,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct Payment {
    pub payer: Address,
    pub amount: i128,
    pub tip: i128,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct AuditEntry {
    pub action: Symbol,
    pub actor: Address,
    pub timestamp: u64,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct SubscriptionParams {
    pub creator: Address,
    pub recipients: Vec<Address>,
    pub amounts: Vec<i128>,
    pub tokens: Vec<Address>,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct CompletionProof {
    pub id: u64,
    pub status: InvoiceStatus,
    pub funded: i128,
    pub timestamp: u64,
    pub hash: BytesN<32>,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct InvoiceTemplate {
    pub recipients: Vec<Address>,
    pub amounts: Vec<i128>,
    pub token: Address,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct CreateInvoiceParams {
    pub recipients: Vec<Address>,
    pub amounts: Vec<i128>,
    pub token: Address,
    pub deadline: u64,
}

/// A single graduated release tranche: `basis_points` out of 10 000 of the
/// invoice total becomes releasable once the ledger time reaches `timestamp`.
#[contracttype]
#[derive(Clone, Debug)]
pub struct Tranche {
    pub timestamp: u64,
    pub basis_points: u32,
}

/// Optional parameters for `create_invoice`, grouped to keep the function
/// within Soroban's 10-parameter limit.
#[contracttype]
#[derive(Clone, Debug)]
pub struct InvoiceOptions {
    pub co_creators: Vec<Address>,
    pub allow_early_withdrawal: bool,
    pub bonus_pool: i128,
    pub bonus_max_payers: u32,
    /// Issue #22: block release until this invoice is Released.
    pub prerequisite_id: Option<u64>,
    /// Issue #23: graduated release schedule; empty = release all at once.
    pub tranches: Vec<Tranche>,
    /// Co-signers whose approval is required before release.
    pub co_signers: Vec<Address>,
    /// How many co-signer approvals are needed (≤ `co_signers.len()`).
    pub required_signatures: u32,
}

/// Legacy invoice layout used by stored invoices created before the `version`
/// field was added. Kept for on-chain migration so old data can be
/// deserialised and re-saved in the current schema.
#[contracttype]
#[derive(Clone, Debug)]
pub struct LegacyInvoice {
    pub creator: Address,
    pub co_creators: Vec<Address>,
    pub recipients: Vec<Address>,
    pub amounts: Vec<i128>,
    pub tokens: Vec<Address>,
    pub deadline: u64,
    pub funded: i128,
    pub status: InvoiceStatus,
    pub payments: Vec<Payment>,
    pub drip_duration: Option<u64>,
    pub release_timestamp: Option<u64>,
    pub claimed: Vec<i128>,
    pub frozen: bool,
    pub completion_time: Option<u64>,
    pub allow_early_withdrawal: bool,
    pub bonus_pool: i128,
    pub bonus_max_payers: u32,
    pub prerequisite_id: Option<u64>,
    pub tranches: Vec<Tranche>,
    pub released_bps: u32,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct Invoice {
    /// Schema version (0 for legacy, 1 for current).
    pub version: u32,
    pub creator: Address,
    pub co_creators: Vec<Address>,
    pub recipients: Vec<Address>,
    pub amounts: Vec<i128>,
    /// Token per recipient (parallel to `recipients`); in practice all entries
    /// are the same token set at creation time.
    pub tokens: Vec<Address>,
    pub deadline: u64,
    pub funded: i128,
    pub status: InvoiceStatus,
    pub payments: Vec<Payment>,
    pub drip_duration: Option<u64>,
    pub release_timestamp: Option<u64>,
    pub claimed: Vec<i128>,
    pub frozen: bool,
    pub completion_time: Option<u64>,
    pub allow_early_withdrawal: bool,
    pub bonus_pool: i128,
    pub bonus_max_payers: u32,
    /// Issue #22: if set, `release()` will fail until this invoice is Released.
    pub prerequisite_id: Option<u64>,
    /// Issue #23: graduated release schedule; empty means release all at once.
    pub tranches: Vec<Tranche>,
    /// Issue #23: cumulative basis points already distributed (0–10 000).
    pub released_bps: u32,
    /// Co-signers that must approve release before funds can be distributed.
    /// If non-empty, `required_signatures` of them must call `sign_release()`.
    pub co_signers: Vec<Address>,
    /// How many co-signer approvals are required to unlock release.
    /// Must be ≤ `co_signers.len()`.
    pub required_signatures: u32,
    /// Co-signers that have already approved release.
    pub signatures: Vec<Address>,
}

impl Invoice {
    /// Upgrade a legacy (pre-version) invoice to the current schema.
    /// New fields are filled with their default (empty / zero) values.
    pub fn from_legacy(old: LegacyInvoice, env: &Env) -> Self {
        Invoice {
            version: 2,
            creator: old.creator,
            co_creators: old.co_creators,
            recipients: old.recipients,
            amounts: old.amounts,
            tokens: old.tokens,
            deadline: old.deadline,
            funded: old.funded,
            status: old.status,
            payments: old.payments,
            drip_duration: old.drip_duration,
            release_timestamp: old.release_timestamp,
            claimed: old.claimed,
            frozen: old.frozen,
            completion_time: old.completion_time,
            allow_early_withdrawal: old.allow_early_withdrawal,
            bonus_pool: old.bonus_pool,
            bonus_max_payers: old.bonus_max_payers,
            prerequisite_id: old.prerequisite_id,
            tranches: old.tranches,
            released_bps: old.released_bps,
            co_signers: Vec::new(env),
            required_signatures: 0,
            signatures: Vec::new(env),
        }
    }
}
