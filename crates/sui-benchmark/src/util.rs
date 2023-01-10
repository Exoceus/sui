// Copyright (c) Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

use anyhow::{Error, Result};
use std::collections::HashMap;
use sui_keys::keystore::{AccountKeystore, FileBasedKeystore};
use sui_types::{base_types::SuiAddress, coin, crypto::SuiKeyPair, SUI_FRAMEWORK_OBJECT_ID};

use crate::ValidatorProxy;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use sui_core::test_utils::{make_transfer_sui_transaction, MAX_GAS};
use sui_types::base_types::{ObjectID, ObjectRef};
use sui_types::messages::{CallArg, ObjectArg, TransactionData, VerifiedTransaction};
use sui_types::object::{Object, Owner};
use sui_types::utils::to_sender_signed_transaction;
use tracing::log::error;

use crate::workloads::workload::{GasCoinConfig, WorkloadInitConfig, WorkloadPayloadConfig};
use sui_types::crypto::{AccountKeyPair, KeypairTraits};

// This is the maximum gas we will transfer from primary coin into any gas coin
// for running the benchmark
pub const MAX_GAS_FOR_TESTING: u64 = 1_000_000_000;

pub type Gas = (ObjectRef, Owner, Arc<AccountKeyPair>);

pub type UpdatedAndNewlyMinted = (ObjectRef, ObjectRef);

pub type UpdatedAndNewlyMintedGasCoins = (Gas, Vec<Gas>);

pub fn get_ed25519_keypair_from_keystore(
    keystore_path: PathBuf,
    requested_address: &SuiAddress,
) -> Result<AccountKeyPair> {
    let keystore = FileBasedKeystore::new(&keystore_path)?;
    match keystore.get_key(requested_address) {
        Ok(SuiKeyPair::Ed25519(kp)) => Ok(kp.copy()),
        other => Err(anyhow::anyhow!("Invalid key type: {:?}", other)),
    }
}

pub async fn transfer_sui_for_testing(
    gas: ObjectRef,
    sender: Owner,
    keypair: &AccountKeyPair,
    value: u64,
    address: SuiAddress,
    proxy: Arc<dyn ValidatorProxy + Sync + Send>,
) -> UpdatedAndNewlyMinted {
    let tx = make_transfer_sui_transaction(
        gas,
        address,
        Some(value),
        sender.get_owner_address().unwrap(),
        keypair,
    );
    // Retry 5 times.
    for _ in 0..5 {
        match proxy.execute_transaction(tx.clone().into()).await {
            Ok((_, effects)) => {
                let minted = effects.created().get(0).unwrap().0;
                let updated = effects
                    .mutated()
                    .iter()
                    .find(|(k, _)| k.0 == gas.0)
                    .unwrap()
                    .0;
                return (updated, minted);
            }
            Err(err) => {
                error!("Error while transferring sui: {:?}", err);
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
        }
    }
    panic!("Failed to finish transfer_sui_for_testing");
}
pub fn make_split_coin_tx(
    framework: ObjectRef,
    sender: SuiAddress,
    coin: &Object,
    split_amounts: Vec<u64>,
    gas: ObjectRef,
    keypair: &AccountKeyPair,
) -> VerifiedTransaction {
    let split_coin = TransactionData::new_move_call(
        sender,
        framework,
        coin::PAY_MODULE_NAME.to_owned(),
        coin::PAY_SPLIT_VEC_FUNC_NAME.to_owned(),
        vec![],
        gas,
        vec![
            CallArg::Object(ObjectArg::ImmOrOwnedObject(coin.compute_object_reference())),
            CallArg::Pure(bcs::to_bytes(&split_amounts).unwrap()),
        ],
        MAX_GAS_FOR_TESTING,
    );
    let verified_tx = to_sender_signed_transaction(split_coin, keypair);
    verified_tx
}

pub fn make_pay_tx(
    input_coins: Vec<ObjectRef>,
    sender: SuiAddress,
    addresses: Vec<SuiAddress>,
    split_amounts: Vec<u64>,
    gas: ObjectRef,
    keypair: &AccountKeyPair,
) -> VerifiedTransaction {
    let pay = TransactionData::new_pay(sender, input_coins, addresses, split_amounts, gas, MAX_GAS);
    let verified_tx = to_sender_signed_transaction(pay, keypair);
    verified_tx
}

pub async fn split_coin_and_pay(
    proxy: Arc<dyn ValidatorProxy + Send + Sync>,
    coin: &Object,
    coin_configs: Vec<GasCoinConfig>,
    gas: Gas,
) -> Result<UpdatedAndNewlyMintedGasCoins> {
    // split one primary gas token into amounts
    // send those amounts to addresses
    let sender = coin.owner.get_owner_address()?;
    let framework = proxy
        .get_object(SUI_FRAMEWORK_OBJECT_ID)
        .await?
        .compute_object_reference();
    let split_amounts: Vec<u64> = coin_configs.iter().map(|c| c.amount).collect();
    let verified_tx = make_split_coin_tx(
        framework,
        sender,
        coin,
        split_amounts.clone(),
        gas.0,
        &gas.2,
    );
    let (_, effects) = proxy.execute_transaction(verified_tx.into()).await?;
    let updated_gas = effects
        .mutated()
        .into_iter()
        .find(|(k, _)| k.0 == gas.0 .0)
        .ok_or("Input gas missing in the effects")
        .map_err(Error::msg)?;
    let created_coins: Vec<ObjectRef> = effects.created().into_iter().map(|c| c.0).collect();
    assert_eq!(created_coins.len(), split_amounts.len());
    let created_coin_ids: Vec<ObjectID> = created_coins.iter().map(|c| c.0).collect();
    let recipient_addresses: Vec<SuiAddress> = coin_configs.iter().map(|x| x.address).collect();
    let verified_tx = make_pay_tx(
        created_coins,
        sender,
        recipient_addresses,
        split_amounts,
        updated_gas.0,
        &gas.2,
    );
    let (_, effects) = proxy.execute_transaction(verified_tx.into()).await?;
    let address_map: HashMap<SuiAddress, Arc<AccountKeyPair>> = coin_configs
        .iter()
        .map(|x| (x.address, x.keypair.clone()))
        .collect();
    let transferred_coins: Result<Vec<Gas>> = effects
        .mutated()
        .into_iter()
        .filter(|(k, _)| created_coin_ids.contains(&(*k).0))
        .map(|c| {
            let address = c.1.get_owner_address()?;
            let keypair = address_map
                .get(&address)
                .ok_or("Owner address missing in the address map")
                .map_err(Error::msg)?;
            Ok((c.0, c.1, keypair.clone()))
        })
        .collect();
    let updated_gas = effects
        .mutated()
        .into_iter()
        .find(|(k, _)| k.0 == gas.0 .0)
        .ok_or("Input gas missing in the effects")
        .map_err(Error::msg)?;
    Ok(((updated_gas.0, updated_gas.1, gas.2), transferred_coins?))
}

pub async fn generate_gas_for_test(
    proxy: Arc<dyn ValidatorProxy + Send + Sync>,
    pay_coin: &Object,
    gas: Gas,
    num_transfer_object_tokens: u64,
    shared_counter_init_gas_config: Vec<GasCoinConfig>,
    shared_counter_payload_coin_configs: Vec<GasCoinConfig>,
    transfer_object_payload_coin_configs: Vec<GasCoinConfig>,
) -> Result<(WorkloadInitConfig, WorkloadPayloadConfig)> {
    let num_shared_counter_init_coins = shared_counter_init_gas_config.len();
    let num_shared_counter_coins =
        num_shared_counter_init_coins + shared_counter_payload_coin_configs.len();

    let mut coin_configs = vec![];
    coin_configs.extend(shared_counter_init_gas_config);
    coin_configs.extend(shared_counter_payload_coin_configs);
    coin_configs.extend(transfer_object_payload_coin_configs);

    let (_updated_primary_gas, new_gas_coins) =
        split_coin_and_pay(proxy.clone(), pay_coin, coin_configs, gas).await?;
    let (shared_counter_coins, transfer_object_coins) =
        new_gas_coins.split_at(num_shared_counter_coins as usize);
    let (shared_counter_init_coins, shared_counter_payload_coins) =
        shared_counter_coins.split_at(num_shared_counter_init_coins as usize);
    let (transfer_tokens, transfer_object_payload_coins) =
        transfer_object_coins.split_at(num_transfer_object_tokens as usize);

    let workload_init_config = WorkloadInitConfig {
        shared_counter_init_gas: shared_counter_init_coins.to_vec(),
    };

    let workload_payload_config = WorkloadPayloadConfig {
        transfer_tokens: transfer_tokens.to_vec(),
        transfer_object_payload_gas: transfer_object_payload_coins.to_vec(),
        shared_counter_payload_gas: shared_counter_payload_coins.to_vec(),
    };
    Ok((workload_init_config, workload_payload_config))
}
