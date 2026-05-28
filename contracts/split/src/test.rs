#![cfg(test)]

use super::*;
use soroban_sdk::{
    testutils::{Address as _, Ledger},
    token::{Client as TokenClient, StellarAssetClient},
    Address, Env, Vec,
};

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

/// Deploy the split contract and a mock USDC token; return (env, contract_id, token_id).
fn setup() -> (Env, Address, Address) {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(SplitContract, ());
    let token_admin = Address::generate(&env);
    let token_id = env.register_stellar_asset_contract_v2(token_admin.clone()).address();

    // Mint tokens to test accounts via the admin interface.
    let stellar_asset = StellarAssetClient::new(&env, &token_id);
    stellar_asset.mint(&token_admin, &1_000_000_000);

    (env, contract_id, token_id)
}

fn client<'a>(env: &'a Env, contract_id: &Address) -> SplitContractClient<'a> {
    SplitContractClient::new(env, contract_id)
}

fn token_client<'a>(env: &'a Env, token_id: &Address) -> TokenClient<'a> {
    TokenClient::new(env, token_id)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn test_create_invoice() {
    let (env, contract_id, token_id) = setup();
    let c = client(&env, &contract_id);

    let creator = Address::generate(&env);
    let recipient = Address::generate(&env);

    let mut recipients = Vec::new(&env);
    recipients.push_back(recipient.clone());
    let mut amounts = Vec::new(&env);
    amounts.push_back(100_i128);

    // Set ledger time so deadline is in the future.
    env.ledger().set_timestamp(1_000);

    let id = c.create_invoice(&creator, &recipients, &amounts, &token_id, &2_000_u64);
    assert_eq!(id, 1);

    let invoice = c.get_invoice(&id);
    assert_eq!(invoice.status, InvoiceStatus::Pending);
    assert_eq!(invoice.funded, 0);
}

#[test]
fn test_pay_and_auto_release() {
    let (env, contract_id, token_id) = setup();
    let c = client(&env, &contract_id);
    let tk = token_client(&env, &token_id);

    let creator = Address::generate(&env);
    let payer = Address::generate(&env);
    let recipient = Address::generate(&env);

    // Fund payer.
    let stellar_asset = StellarAssetClient::new(&env, &token_id);
    stellar_asset.mint(&payer, &500);

    env.ledger().set_timestamp(1_000);

    let mut recipients = Vec::new(&env);
    recipients.push_back(recipient.clone());
    let mut amounts = Vec::new(&env);
    amounts.push_back(200_i128);

    let id = c.create_invoice(&creator, &recipients, &amounts, &token_id, &9_999_u64);

    // Pay full amount — should auto-release.
    c.pay(&payer, &id, &200_i128);

    let invoice = c.get_invoice(&id);
    assert_eq!(invoice.status, InvoiceStatus::Released);

    // Recipient should have received 200.
    assert_eq!(tk.balance(&recipient), 200);
}

#[test]
fn test_partial_pay_then_release() {
    let (env, contract_id, token_id) = setup();
    let c = client(&env, &contract_id);
    let tk = token_client(&env, &token_id);

    let creator = Address::generate(&env);
    let payer1 = Address::generate(&env);
    let payer2 = Address::generate(&env);
    let recipient = Address::generate(&env);

    let stellar_asset = StellarAssetClient::new(&env, &token_id);
    stellar_asset.mint(&payer1, &150);
    stellar_asset.mint(&payer2, &150);

    env.ledger().set_timestamp(1_000);

    let mut recipients = Vec::new(&env);
    recipients.push_back(recipient.clone());
    let mut amounts = Vec::new(&env);
    amounts.push_back(300_i128);

    let id = c.create_invoice(&creator, &recipients, &amounts, &token_id, &9_999_u64);

    c.pay(&payer1, &id, &150_i128);
    let invoice = c.get_invoice(&id);
    assert_eq!(invoice.status, InvoiceStatus::Pending);

    c.pay(&payer2, &id, &150_i128);
    let invoice = c.get_invoice(&id);
    assert_eq!(invoice.status, InvoiceStatus::Released);
    assert_eq!(tk.balance(&recipient), 300);
}

#[test]
fn test_refund_after_deadline() {
    let (env, contract_id, token_id) = setup();
    let c = client(&env, &contract_id);
    let tk = token_client(&env, &token_id);

    let creator = Address::generate(&env);
    let payer = Address::generate(&env);
    let recipient = Address::generate(&env);

    let stellar_asset = StellarAssetClient::new(&env, &token_id);
    stellar_asset.mint(&payer, &100);

    env.ledger().set_timestamp(1_000);

    let mut recipients = Vec::new(&env);
    recipients.push_back(recipient.clone());
    let mut amounts = Vec::new(&env);
    amounts.push_back(500_i128);

    let id = c.create_invoice(&creator, &recipients, &amounts, &token_id, &2_000_u64);

    // Partial payment.
    c.pay(&payer, &id, &100_i128);

    // Advance past deadline.
    env.ledger().set_timestamp(3_000);

    c.refund(&id);

    let invoice = c.get_invoice(&id);
    assert_eq!(invoice.status, InvoiceStatus::Refunded);
    // Payer should be refunded.
    assert_eq!(tk.balance(&payer), 100);
}

#[test]
#[should_panic(expected = "invoice deadline has passed")]
fn test_pay_after_deadline_panics() {
    let (env, contract_id, token_id) = setup();
    let c = client(&env, &contract_id);

    let creator = Address::generate(&env);
    let payer = Address::generate(&env);
    let recipient = Address::generate(&env);

    let stellar_asset = StellarAssetClient::new(&env, &token_id);
    stellar_asset.mint(&payer, &100);

    env.ledger().set_timestamp(1_000);

    let mut recipients = Vec::new(&env);
    recipients.push_back(recipient.clone());
    let mut amounts = Vec::new(&env);
    amounts.push_back(100_i128);

    let id = c.create_invoice(&creator, &recipients, &amounts, &token_id, &2_000_u64);

    env.ledger().set_timestamp(3_000);
    c.pay(&payer, &id, &100_i128);
}

#[test]
#[should_panic(expected = "payment exceeds remaining balance")]
fn test_overpayment_panics() {
    let (env, contract_id, token_id) = setup();
    let c = client(&env, &contract_id);

    let creator = Address::generate(&env);
    let payer = Address::generate(&env);
    let recipient = Address::generate(&env);

    let stellar_asset = StellarAssetClient::new(&env, &token_id);
    stellar_asset.mint(&payer, &1_000);

    env.ledger().set_timestamp(1_000);

    let mut recipients = Vec::new(&env);
    recipients.push_back(recipient.clone());
    let mut amounts = Vec::new(&env);
    amounts.push_back(100_i128);

    let id = c.create_invoice(&creator, &recipients, &amounts, &token_id, &9_999_u64);
    c.pay(&payer, &id, &200_i128);
}

#[test]
fn test_multi_recipient_release() {
    let (env, contract_id, token_id) = setup();
    let c = client(&env, &contract_id);
    let tk = token_client(&env, &token_id);

    let creator = Address::generate(&env);
    let payer = Address::generate(&env);
    let r1 = Address::generate(&env);
    let r2 = Address::generate(&env);
    let r3 = Address::generate(&env);

    let stellar_asset = StellarAssetClient::new(&env, &token_id);
    stellar_asset.mint(&payer, &600);

    env.ledger().set_timestamp(1_000);

    let mut recipients = Vec::new(&env);
    recipients.push_back(r1.clone());
    recipients.push_back(r2.clone());
    recipients.push_back(r3.clone());

    let mut amounts = Vec::new(&env);
    amounts.push_back(100_i128);
    amounts.push_back(200_i128);
    amounts.push_back(300_i128);

    let id = c.create_invoice(&creator, &recipients, &amounts, &token_id, &9_999_u64);
    c.pay(&payer, &id, &600_i128);

    assert_eq!(tk.balance(&r1), 100);
    assert_eq!(tk.balance(&r2), 200);
    assert_eq!(tk.balance(&r3), 300);
}

#[test]
fn test_audit_log() {
    let (env, contract_id, token_id) = setup();
    let c = client(&env, &contract_id);
    let stellar_asset = StellarAssetClient::new(&env, &token_id);

    let creator = Address::generate(&env);
    let payer = Address::generate(&env);
    let recipient = Address::generate(&env);

    stellar_asset.mint(&payer, &500);

    env.ledger().set_timestamp(1_000);

    let mut recipients = Vec::new(&env);
    recipients.push_back(recipient.clone());
    let mut amounts = Vec::new(&env);
    amounts.push_back(200_i128);

    let id = c.create_invoice(&creator, &recipients, &amounts, &token_id, &9_999_u64);

    // Perform action: pay (auto-release)
    c.pay(&payer, &id, &200_i128);

    let invoice = c.get_invoice(&id);
    assert_eq!(invoice.status, InvoiceStatus::Released);

    // Check audit log has 2 entries (pay and release)
    let log = c.get_audit_log(&id);
    assert_eq!(log.len(), 2);
    assert_eq!(log.get_unchecked(0).action, symbol_short!("pay"));
    assert_eq!(log.get_unchecked(1).action, symbol_short!("release"));
}

#[test]
fn test_audit_log_with_cancel() {
    let (env, contract_id, token_id) = setup();
    let c = client(&env, &contract_id);

    let creator = Address::generate(&env);
    let recipient = Address::generate(&env);

    env.ledger().set_timestamp(1_000);

    let mut recipients = Vec::new(&env);
    recipients.push_back(recipient.clone());
    let mut amounts = Vec::new(&env);
    amounts.push_back(100_i128);

    let id = c.create_invoice(&creator, &recipients, &amounts, &token_id, &9_999_u64);

    // Cancel the invoice
    c.cancel_invoice(&creator, &id);

    // Check audit log has 1 entry (cancel)
    let log = c.get_audit_log(&id);
    assert_eq!(log.len(), 1);
    assert_eq!(log.get_unchecked(0).action, symbol_short!("cancel"));
    assert_eq!(log.get_unchecked(0).actor, creator);
}

#[test]
fn test_audit_log_with_extend() {
    let (env, contract_id, token_id) = setup();
    let c = client(&env, &contract_id);

    let creator = Address::generate(&env);
    let recipient = Address::generate(&env);

    env.ledger().set_timestamp(1_000);

    let mut recipients = Vec::new(&env);
    recipients.push_back(recipient.clone());
    let mut amounts = Vec::new(&env);
    amounts.push_back(100_i128);

    let id = c.create_invoice(&creator, &recipients, &amounts, &token_id, &2_000_u64);

    // Extend the deadline
    c.extend_deadline(&creator, &id, &9_999_u64);

    // Check audit log has 1 entry (extend)
    let log = c.get_audit_log(&id);
    assert_eq!(log.len(), 1);
    assert_eq!(log.get_unchecked(0).action, symbol_short!("extend"));
    assert_eq!(log.get_unchecked(0).actor, creator);
}

#[test]
fn test_create_subscription() {
    let (env, contract_id, token_id) = setup();
    let c = client(&env, &contract_id);
    let tk = token_client(&env, &token_id);
    let stellar_asset = StellarAssetClient::new(&env, &token_id);

    let creator = Address::generate(&env);
    let payer = Address::generate(&env);
    let recipient = Address::generate(&env);

    stellar_asset.mint(&payer, &500);

    env.ledger().set_timestamp(1_000);

    let mut recipients = Vec::new(&env);
    recipients.push_back(recipient.clone());
    let mut amounts = Vec::new(&env);
    amounts.push_back(200_i128);

    // Create a 3-month subscription
    let id = c.create_subscription(&creator, &recipients, &amounts, &token_id, &3_u32);
    assert_eq!(id, 1);

    // Pay and auto-release first invoice
    c.pay(&payer, &id, &200_i128);

    let invoice = c.get_invoice(&id);
    assert_eq!(invoice.status, InvoiceStatus::Released);

    // Second invoice should be created automatically (30 days after release)
    let second_invoice = c.get_invoice(&2);
    assert_eq!(second_invoice.status, InvoiceStatus::Pending);

    // Recipient should have received 200 from first invoice
    assert_eq!(tk.balance(&recipient), 200);
}

// ---------------------------------------------------------------------------
// Installment plan tests
// ---------------------------------------------------------------------------

#[test]
fn test_register_and_get_installment_plan() {
    let (env, contract_id, token_id) = setup();
    let c = client(&env, &contract_id);

    let creator = Address::generate(&env);
    let payer = Address::generate(&env);
    let recipient = Address::generate(&env);

    env.ledger().set_timestamp(1_000);

    let mut recipients = Vec::new(&env);
    recipients.push_back(recipient.clone());
    let mut amounts = Vec::new(&env);
    amounts.push_back(300_i128);

    let id = c.create_invoice(&creator, &recipients, &amounts, &token_id, &9_999_u64);

    // Register a 3-installment plan.
    let mut schedule: Vec<(u64, i128)> = Vec::new(&env);
    schedule.push_back((2_000_u64, 100_i128));
    schedule.push_back((5_000_u64, 100_i128));
    schedule.push_back((8_000_u64, 100_i128));

    c.register_installment_plan(&payer, &id, &schedule);

    let retrieved = c.get_installment_plan(&payer, &id);
    assert_eq!(retrieved.len(), 3);
    assert_eq!(retrieved.get_unchecked(0), (2_000_u64, 100_i128));
    assert_eq!(retrieved.get_unchecked(2), (8_000_u64, 100_i128));
}

#[test]
fn test_installment_plan_can_be_updated() {
    let (env, contract_id, token_id) = setup();
    let c = client(&env, &contract_id);

    let creator = Address::generate(&env);
    let payer = Address::generate(&env);
    let recipient = Address::generate(&env);

    env.ledger().set_timestamp(1_000);

    let mut recipients = Vec::new(&env);
    recipients.push_back(recipient.clone());
    let mut amounts = Vec::new(&env);
    amounts.push_back(200_i128);

    let id = c.create_invoice(&creator, &recipients, &amounts, &token_id, &9_999_u64);

    let mut schedule1: Vec<(u64, i128)> = Vec::new(&env);
    schedule1.push_back((3_000_u64, 200_i128));
    c.register_installment_plan(&payer, &id, &schedule1);

    // Overwrite with a 2-installment plan.
    let mut schedule2: Vec<(u64, i128)> = Vec::new(&env);
    schedule2.push_back((3_000_u64, 100_i128));
    schedule2.push_back((6_000_u64, 100_i128));
    c.register_installment_plan(&payer, &id, &schedule2);

    let retrieved = c.get_installment_plan(&payer, &id);
    assert_eq!(retrieved.len(), 2);
}

#[test]
fn test_pay_unaffected_by_installment_plan() {
    let (env, contract_id, token_id) = setup();
    let c = client(&env, &contract_id);
    let tk = token_client(&env, &token_id);
    let stellar_asset = StellarAssetClient::new(&env, &token_id);

    let creator = Address::generate(&env);
    let payer = Address::generate(&env);
    let recipient = Address::generate(&env);

    stellar_asset.mint(&payer, &200);
    env.ledger().set_timestamp(1_000);

    let mut recipients = Vec::new(&env);
    recipients.push_back(recipient.clone());
    let mut amounts = Vec::new(&env);
    amounts.push_back(200_i128);

    let id = c.create_invoice(&creator, &recipients, &amounts, &token_id, &9_999_u64);

    let mut schedule: Vec<(u64, i128)> = Vec::new(&env);
    schedule.push_back((5_000_u64, 200_i128));
    c.register_installment_plan(&payer, &id, &schedule);

    // Pay in full immediately — plan doesn't restrict this.
    c.pay(&payer, &id, &200_i128);

    let invoice = c.get_invoice(&id);
    assert_eq!(invoice.status, InvoiceStatus::Released);
    assert_eq!(tk.balance(&recipient), 200);
}

// ---------------------------------------------------------------------------
// Token whitelist tests
// ---------------------------------------------------------------------------

#[test]
fn test_whitelisted_token_creates_invoice() {
    let (env, contract_id, token_id) = setup();
    let c = client(&env, &contract_id);

    let admin = Address::generate(&env);
    let creator = Address::generate(&env);
    let recipient = Address::generate(&env);

    c.init_admin(&admin);
    c.whitelist_token(&token_id);

    env.ledger().set_timestamp(1_000);

    let mut recipients = Vec::new(&env);
    recipients.push_back(recipient.clone());
    let mut amounts = Vec::new(&env);
    amounts.push_back(100_i128);

    let id = c.create_invoice(&creator, &recipients, &amounts, &token_id, &9_999_u64);
    assert_eq!(id, 1);
}

#[test]
#[should_panic(expected = "token not whitelisted")]
fn test_non_whitelisted_token_panics() {
    let (env, contract_id, token_id) = setup();
    let c = client(&env, &contract_id);

    let admin = Address::generate(&env);
    let creator = Address::generate(&env);
    let recipient = Address::generate(&env);

    // Init admin and whitelist a *different* token (not token_id).
    let other_token = Address::generate(&env);
    c.init_admin(&admin);
    c.whitelist_token(&other_token);

    env.ledger().set_timestamp(1_000);

    let mut recipients = Vec::new(&env);
    recipients.push_back(recipient.clone());
    let mut amounts = Vec::new(&env);
    amounts.push_back(100_i128);

    c.create_invoice(&creator, &recipients, &amounts, &token_id, &9_999_u64);
}

#[test]
#[should_panic(expected = "token not whitelisted")]
fn test_remove_token_from_whitelist() {
    let (env, contract_id, token_id) = setup();
    let c = client(&env, &contract_id);

    let admin = Address::generate(&env);
    let creator = Address::generate(&env);
    let recipient = Address::generate(&env);

    c.init_admin(&admin);
    c.whitelist_token(&token_id);
    c.remove_token(&token_id);

    env.ledger().set_timestamp(1_000);

    let mut recipients = Vec::new(&env);
    recipients.push_back(recipient.clone());
    let mut amounts = Vec::new(&env);
    amounts.push_back(100_i128);

    // After removal, create_invoice should panic.
    c.create_invoice(&creator, &recipients, &amounts, &token_id, &9_999_u64);
}

// ---------------------------------------------------------------------------
// Recurring invoice tests
// ---------------------------------------------------------------------------

#[test]
fn test_recurring_invoice_creates_next_on_release() {
    let (env, contract_id, token_id) = setup();
    let c = client(&env, &contract_id);
    let tk = token_client(&env, &token_id);
    let stellar_asset = StellarAssetClient::new(&env, &token_id);

    let creator = Address::generate(&env);
    let payer = Address::generate(&env);
    let recipient = Address::generate(&env);

    stellar_asset.mint(&payer, &400);
    env.ledger().set_timestamp(1_000);

    let mut recipients = Vec::new(&env);
    recipients.push_back(recipient.clone());
    let mut amounts = Vec::new(&env);
    amounts.push_back(200_i128);

    // deadline = 9_999, original_duration = 9_999 - 1_000 = 8_999
    let id = c.create_invoice_recurring(
        &creator,
        &recipients,
        &amounts,
        &token_id,
        &9_999_u64,
        &true,
    );
    assert_eq!(id, 1);

    c.pay(&payer, &id, &200_i128);

    let first = c.get_invoice(&id);
    assert_eq!(first.status, InvoiceStatus::Released);

    // New invoice auto-created with id=2.
    let second = c.get_invoice(&2);
    assert_eq!(second.status, InvoiceStatus::Pending);
    // New deadline = release_timestamp + original_duration = 1_000 + 8_999 = 9_999
    assert_eq!(second.deadline, 1_000 + 8_999);
    assert_eq!(second.recurring, true);

    // Recipient received funds from first invoice.
    assert_eq!(tk.balance(&recipient), 200);
}

#[test]
fn test_non_recurring_invoice_no_auto_create() {
    let (env, contract_id, token_id) = setup();
    let c = client(&env, &contract_id);
    let stellar_asset = StellarAssetClient::new(&env, &token_id);

    let creator = Address::generate(&env);
    let payer = Address::generate(&env);
    let recipient = Address::generate(&env);

    stellar_asset.mint(&payer, &200);
    env.ledger().set_timestamp(1_000);

    let mut recipients = Vec::new(&env);
    recipients.push_back(recipient.clone());
    let mut amounts = Vec::new(&env);
    amounts.push_back(200_i128);

    let id = c.create_invoice(&creator, &recipients, &amounts, &token_id, &9_999_u64);
    c.pay(&payer, &id, &200_i128);

    let first = c.get_invoice(&id);
    assert_eq!(first.status, InvoiceStatus::Released);
    // No second invoice — counter stays at 1.
    assert_eq!(first.recurring, false);
}

// ---------------------------------------------------------------------------
// batch_pay tests
// ---------------------------------------------------------------------------

#[test]
fn test_batch_pay_three_invoices() {
    let (env, contract_id, token_id) = setup();
    let c = client(&env, &contract_id);
    let tk = token_client(&env, &token_id);
    let stellar_asset = StellarAssetClient::new(&env, &token_id);

    let creator = Address::generate(&env);
    let payer = Address::generate(&env);
    let r1 = Address::generate(&env);
    let r2 = Address::generate(&env);
    let r3 = Address::generate(&env);

    stellar_asset.mint(&payer, &600);
    env.ledger().set_timestamp(1_000);

    // Create 3 invoices.
    let id1 = {
        let mut rs = Vec::new(&env);
        rs.push_back(r1.clone());
        let mut am = Vec::new(&env);
        am.push_back(100_i128);
        c.create_invoice(&creator, &rs, &am, &token_id, &9_999_u64)
    };
    let id2 = {
        let mut rs = Vec::new(&env);
        rs.push_back(r2.clone());
        let mut am = Vec::new(&env);
        am.push_back(200_i128);
        c.create_invoice(&creator, &rs, &am, &token_id, &9_999_u64)
    };
    let id3 = {
        let mut rs = Vec::new(&env);
        rs.push_back(r3.clone());
        let mut am = Vec::new(&env);
        am.push_back(300_i128);
        c.create_invoice(&creator, &rs, &am, &token_id, &9_999_u64)
    };

    let mut payments: Vec<(u64, i128)> = Vec::new(&env);
    payments.push_back((id1, 100_i128));
    payments.push_back((id2, 200_i128));
    payments.push_back((id3, 300_i128));

    c.batch_pay(&payer, &payments);

    assert_eq!(c.get_invoice(&id1).status, InvoiceStatus::Released);
    assert_eq!(c.get_invoice(&id2).status, InvoiceStatus::Released);
    assert_eq!(c.get_invoice(&id3).status, InvoiceStatus::Released);
    assert_eq!(tk.balance(&r1), 100);
    assert_eq!(tk.balance(&r2), 200);
    assert_eq!(tk.balance(&r3), 300);
}

#[test]
fn test_batch_pay_empty_is_noop() {
    let (env, contract_id, _token_id) = setup();
    let c = client(&env, &contract_id);

    let payer = Address::generate(&env);
    let payments: Vec<(u64, i128)> = Vec::new(&env);

    // Should not panic.
    c.batch_pay(&payer, &payments);
}

#[test]
#[should_panic(expected = "invoice is not pending")]
fn test_batch_pay_reverts_on_invalid_payment() {
    let (env, contract_id, token_id) = setup();
    let c = client(&env, &contract_id);
    let stellar_asset = StellarAssetClient::new(&env, &token_id);

    let creator = Address::generate(&env);
    let payer = Address::generate(&env);
    let recipient = Address::generate(&env);

    stellar_asset.mint(&payer, &500);
    env.ledger().set_timestamp(1_000);

    let mut rs = Vec::new(&env);
    rs.push_back(recipient.clone());
    let mut am = Vec::new(&env);
    am.push_back(100_i128);

    let id1 = c.create_invoice(&creator, &rs, &am, &token_id, &9_999_u64);
    // Release id1 first so it's no longer pending.
    c.pay(&payer, &id1, &100_i128);

    let id2 = c.create_invoice(&creator, &rs, &am, &token_id, &9_999_u64);

    let mut payments: Vec<(u64, i128)> = Vec::new(&env);
    payments.push_back((id2, 100_i128));
    payments.push_back((id1, 100_i128)); // id1 is Released — should panic

    c.batch_pay(&payer, &payments);
}