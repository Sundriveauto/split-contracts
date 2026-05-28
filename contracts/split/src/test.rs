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
    let token_id = env
        .register_stellar_asset_contract_v2(token_admin.clone())
        .address();

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

/// Build a single-element tokens Vec from one token address.
fn single_tokens(env: &Env, token: &Address) -> Vec<Address> {
    let mut v = Vec::new(env);
    v.push_back(token.clone());
    v
}

// ---------------------------------------------------------------------------
// Existing tests (updated for new tokens parameter)
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
    let tokens = single_tokens(&env, &token_id);

    env.ledger().set_timestamp(1_000);

    let id = c.create_invoice(&creator, &recipients, &amounts, &tokens, &2_000_u64);
    assert_eq!(id, 1);

    let invoice = c.get_invoice(&id);
    assert_eq!(invoice.status, InvoiceStatus::Pending);
    assert_eq!(invoice.funded, 0);
    assert!(!invoice.frozen);
}

#[test]
fn test_pay_and_auto_release() {
    let (env, contract_id, token_id) = setup();
    let c = client(&env, &contract_id);
    let tk = token_client(&env, &token_id);

    let creator = Address::generate(&env);
    let payer = Address::generate(&env);
    let recipient = Address::generate(&env);

    let stellar_asset = StellarAssetClient::new(&env, &token_id);
    stellar_asset.mint(&payer, &500);

    env.ledger().set_timestamp(1_000);

    let mut recipients = Vec::new(&env);
    recipients.push_back(recipient.clone());
    let mut amounts = Vec::new(&env);
    amounts.push_back(200_i128);
    let tokens = single_tokens(&env, &token_id);

    let id = c.create_invoice(&creator, &recipients, &amounts, &tokens, &9_999_u64);

    c.pay(&payer, &id, &200_i128);

    let invoice = c.get_invoice(&id);
    assert_eq!(invoice.status, InvoiceStatus::Released);
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
    let tokens = single_tokens(&env, &token_id);

    let id = c.create_invoice(&creator, &recipients, &amounts, &tokens, &9_999_u64);

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
    let tokens = single_tokens(&env, &token_id);

    let id = c.create_invoice(&creator, &recipients, &amounts, &tokens, &2_000_u64);

    c.pay(&payer, &id, &100_i128);

    env.ledger().set_timestamp(3_000);

    c.refund(&id);

    let invoice = c.get_invoice(&id);
    assert_eq!(invoice.status, InvoiceStatus::Refunded);
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
    let tokens = single_tokens(&env, &token_id);

    let id = c.create_invoice(&creator, &recipients, &amounts, &tokens, &2_000_u64);

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
    let tokens = single_tokens(&env, &token_id);

    let id = c.create_invoice(&creator, &recipients, &amounts, &tokens, &9_999_u64);
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

    let mut tokens = Vec::new(&env);
    tokens.push_back(token_id.clone());
    tokens.push_back(token_id.clone());
    tokens.push_back(token_id.clone());

    let id = c.create_invoice(&creator, &recipients, &amounts, &tokens, &9_999_u64);
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
    let tokens = single_tokens(&env, &token_id);

    let id = c.create_invoice(&creator, &recipients, &amounts, &tokens, &9_999_u64);

    c.pay(&payer, &id, &200_i128);

    let invoice = c.get_invoice(&id);
    assert_eq!(invoice.status, InvoiceStatus::Released);

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
    let tokens = single_tokens(&env, &token_id);

    let id = c.create_invoice(&creator, &recipients, &amounts, &tokens, &9_999_u64);

    c.cancel_invoice(&creator, &id);

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
    let tokens = single_tokens(&env, &token_id);

    let id = c.create_invoice(&creator, &recipients, &amounts, &tokens, &2_000_u64);

    c.extend_deadline(&creator, &id, &9_999_u64);

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
    let tokens = single_tokens(&env, &token_id);

    let id = c.create_subscription(&creator, &recipients, &amounts, &tokens, &3_u32);
    assert_eq!(id, 1);

    c.pay(&payer, &id, &200_i128);

    let invoice = c.get_invoice(&id);
    assert_eq!(invoice.status, InvoiceStatus::Released);

    let second_invoice = c.get_invoice(&2);
    assert_eq!(second_invoice.status, InvoiceStatus::Pending);

    assert_eq!(tk.balance(&recipient), 200);
}

// ---------------------------------------------------------------------------
// New feature tests
// ---------------------------------------------------------------------------

/// Multi-token: two recipients each paid in a different token.
#[test]
fn test_multi_token_invoice() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(SplitContract, ());

    // Create two separate token contracts.
    let token_admin = Address::generate(&env);
    let token_a_id = env
        .register_stellar_asset_contract_v2(token_admin.clone())
        .address();
    let token_b_id = env
        .register_stellar_asset_contract_v2(token_admin.clone())
        .address();

    let sa_a = StellarAssetClient::new(&env, &token_a_id);
    let sa_b = StellarAssetClient::new(&env, &token_b_id);

    let creator = Address::generate(&env);
    let payer = Address::generate(&env);
    let r1 = Address::generate(&env);
    let r2 = Address::generate(&env);

    // Payer funds in token_a (used for payment collection).
    sa_a.mint(&payer, &300);
    // Contract must hold token_b to pay r2 (pre-fund for test).
    sa_b.mint(&contract_id, &200);

    env.ledger().set_timestamp(1_000);

    let c = SplitContractClient::new(&env, &contract_id);

    let mut recipients = Vec::new(&env);
    recipients.push_back(r1.clone());
    recipients.push_back(r2.clone());

    let mut amounts = Vec::new(&env);
    amounts.push_back(100_i128); // r1 gets 100 token_a
    amounts.push_back(200_i128); // r2 gets 200 token_b

    let mut tokens = Vec::new(&env);
    tokens.push_back(token_a_id.clone());
    tokens.push_back(token_b_id.clone());

    let id = c.create_invoice(&creator, &recipients, &amounts, &tokens, &9_999_u64);

    // Pay total (300) in token_a (tokens[0]).
    c.pay(&payer, &id, &300_i128);

    let invoice = c.get_invoice(&id);
    assert_eq!(invoice.status, InvoiceStatus::Released);

    // r1 received 100 token_a, r2 received 200 token_b.
    assert_eq!(TokenClient::new(&env, &token_a_id).balance(&r1), 100);
    assert_eq!(TokenClient::new(&env, &token_b_id).balance(&r2), 200);
}

/// Multi-token: mismatched lengths panic.
#[test]
#[should_panic(expected = "recipients and tokens length mismatch")]
fn test_multi_token_length_mismatch_panics() {
    let (env, contract_id, token_id) = setup();
    let c = client(&env, &contract_id);

    let creator = Address::generate(&env);
    let r1 = Address::generate(&env);

    env.ledger().set_timestamp(1_000);

    let mut recipients = Vec::new(&env);
    recipients.push_back(r1.clone());
    let mut amounts = Vec::new(&env);
    amounts.push_back(100_i128);
    // Empty tokens vec — mismatch.
    let tokens: Vec<Address> = Vec::new(&env);

    c.create_invoice(&creator, &recipients, &amounts, &tokens, &9_999_u64);
}

/// Protocol fee: treasury receives fee_bps% of total, recipients get remainder.
#[test]
fn test_protocol_fee() {
    let (env, contract_id, token_id) = setup();
    let c = client(&env, &contract_id);
    let tk = token_client(&env, &token_id);

    let admin = Address::generate(&env);
    let treasury = Address::generate(&env);
    let creator = Address::generate(&env);
    let payer = Address::generate(&env);
    let recipient = Address::generate(&env);

    let stellar_asset = StellarAssetClient::new(&env, &token_id);
    stellar_asset.mint(&payer, &1_000);

    // Initialize with 100 bps (1%) fee.
    c.initialize(&admin, &treasury, &100_u32);

    env.ledger().set_timestamp(1_000);

    let mut recipients = Vec::new(&env);
    recipients.push_back(recipient.clone());
    let mut amounts = Vec::new(&env);
    amounts.push_back(1_000_i128);
    let tokens = single_tokens(&env, &token_id);

    let id = c.create_invoice(&creator, &recipients, &amounts, &tokens, &9_999_u64);
    c.pay(&payer, &id, &1_000_i128);

    // fee = 1000 * 100 / 10000 = 10
    assert_eq!(tk.balance(&treasury), 10);
    // recipient gets post-fee amount: 1000 - 10 = 990
    assert_eq!(tk.balance(&recipient), 990);
}

/// Protocol fee: fee_bps = 0 behaves identically to no fee.
#[test]
fn test_protocol_fee_zero() {
    let (env, contract_id, token_id) = setup();
    let c = client(&env, &contract_id);
    let tk = token_client(&env, &token_id);

    let admin = Address::generate(&env);
    let treasury = Address::generate(&env);
    let creator = Address::generate(&env);
    let payer = Address::generate(&env);
    let recipient = Address::generate(&env);

    let stellar_asset = StellarAssetClient::new(&env, &token_id);
    stellar_asset.mint(&payer, &500);

    c.initialize(&admin, &treasury, &0_u32);

    env.ledger().set_timestamp(1_000);

    let mut recipients = Vec::new(&env);
    recipients.push_back(recipient.clone());
    let mut amounts = Vec::new(&env);
    amounts.push_back(500_i128);
    let tokens = single_tokens(&env, &token_id);

    let id = c.create_invoice(&creator, &recipients, &amounts, &tokens, &9_999_u64);
    c.pay(&payer, &id, &500_i128);

    assert_eq!(tk.balance(&treasury), 0);
    assert_eq!(tk.balance(&recipient), 500);
}

/// Freeze/unfreeze: pay on frozen invoice panics.
#[test]
#[should_panic(expected = "invoice is frozen")]
fn test_pay_frozen_invoice_panics() {
    let (env, contract_id, token_id) = setup();
    let c = client(&env, &contract_id);

    let admin = Address::generate(&env);
    let treasury = Address::generate(&env);
    let creator = Address::generate(&env);
    let payer = Address::generate(&env);
    let recipient = Address::generate(&env);

    let stellar_asset = StellarAssetClient::new(&env, &token_id);
    stellar_asset.mint(&payer, &200);

    c.initialize(&admin, &treasury, &0_u32);

    env.ledger().set_timestamp(1_000);

    let mut recipients = Vec::new(&env);
    recipients.push_back(recipient.clone());
    let mut amounts = Vec::new(&env);
    amounts.push_back(200_i128);
    let tokens = single_tokens(&env, &token_id);

    let id = c.create_invoice(&creator, &recipients, &amounts, &tokens, &9_999_u64);

    c.freeze_invoice(&id);
    c.pay(&payer, &id, &200_i128); // should panic
}

/// Freeze/unfreeze: unfreeze restores normal operation.
#[test]
fn test_freeze_unfreeze_invoice() {
    let (env, contract_id, token_id) = setup();
    let c = client(&env, &contract_id);
    let tk = token_client(&env, &token_id);

    let admin = Address::generate(&env);
    let treasury = Address::generate(&env);
    let creator = Address::generate(&env);
    let payer = Address::generate(&env);
    let recipient = Address::generate(&env);

    let stellar_asset = StellarAssetClient::new(&env, &token_id);
    stellar_asset.mint(&payer, &200);

    c.initialize(&admin, &treasury, &0_u32);

    env.ledger().set_timestamp(1_000);

    let mut recipients = Vec::new(&env);
    recipients.push_back(recipient.clone());
    let mut amounts = Vec::new(&env);
    amounts.push_back(200_i128);
    let tokens = single_tokens(&env, &token_id);

    let id = c.create_invoice(&creator, &recipients, &amounts, &tokens, &9_999_u64);

    c.freeze_invoice(&id);
    assert!(c.get_invoice(&id).frozen);

    c.unfreeze_invoice(&id);
    assert!(!c.get_invoice(&id).frozen);

    // Payment should succeed after unfreeze.
    c.pay(&payer, &id, &200_i128);
    assert_eq!(c.get_invoice(&id).status, InvoiceStatus::Released);
    assert_eq!(tk.balance(&recipient), 200);
}

/// Creator index: 3 invoices from same creator all returned.
#[test]
fn test_creator_index() {
    let (env, contract_id, token_id) = setup();
    let c = client(&env, &contract_id);

    let creator = Address::generate(&env);
    let other = Address::generate(&env);
    let recipient = Address::generate(&env);

    env.ledger().set_timestamp(1_000);

    let mut recipients = Vec::new(&env);
    recipients.push_back(recipient.clone());
    let mut amounts = Vec::new(&env);
    amounts.push_back(100_i128);
    let tokens = single_tokens(&env, &token_id);

    let id1 = c.create_invoice(&creator, &recipients, &amounts, &tokens, &9_999_u64);
    let id2 = c.create_invoice(&creator, &recipients, &amounts, &tokens, &9_999_u64);
    let id3 = c.create_invoice(&creator, &recipients, &amounts, &tokens, &9_999_u64);
    // Different creator — should not appear in creator's index.
    c.create_invoice(&other, &recipients, &amounts, &tokens, &9_999_u64);

    let ids = c.get_invoices_by_creator(&creator);
    assert_eq!(ids.len(), 3);
    assert_eq!(ids.get_unchecked(0), id1);
    assert_eq!(ids.get_unchecked(1), id2);
    assert_eq!(ids.get_unchecked(2), id3);
}

/// Creator index: returns empty Vec for address with no invoices.
#[test]
fn test_creator_index_empty() {
    let (env, contract_id, _token_id) = setup();
    let c = client(&env, &contract_id);

    let nobody = Address::generate(&env);
    let ids = c.get_invoices_by_creator(&nobody);
    assert_eq!(ids.len(), 0);
}
