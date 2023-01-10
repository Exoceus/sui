// Copyright (c) Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

use async_trait::async_trait;
use itertools::Itertools;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::info;

use sui_types::{
    base_types::{ObjectID, ObjectRef, SuiAddress},
    crypto::{get_key_pair, AccountKeyPair},
    messages::VerifiedTransaction,
    object::Owner,
};

use crate::util::Gas;
use crate::workloads::workload::{GasCoinConfig, WorkloadInitConfig, WorkloadPayloadConfig};
use crate::ValidatorProxy;
use sui_core::test_utils::make_transfer_object_transaction;

use super::workload::{Payload, Workload, WorkloadType, MAX_GAS_FOR_TESTING};

pub struct TransferObjectTestPayload {
    transfer_object: ObjectRef,
    transfer_from: SuiAddress,
    transfer_to: SuiAddress,
    gas: Vec<Gas>,
    keypairs: Arc<HashMap<SuiAddress, AccountKeyPair>>,
}

impl Payload for TransferObjectTestPayload {
    fn make_new_payload(
        self: Box<Self>,
        new_object: ObjectRef,
        new_gas: ObjectRef,
    ) -> Box<dyn Payload> {
        let recipient = self
            .gas
            .iter()
            .find(|x| x.1.get_owner_address().unwrap() != self.transfer_to)
            .unwrap()
            .1
            .clone();
        let updated_gas: Vec<Gas> = self
            .gas
            .into_iter()
            .map(|x| {
                if x.1.get_owner_address().unwrap() == self.transfer_from {
                    (new_gas, Owner::AddressOwner(self.transfer_from), x.2)
                } else {
                    x
                }
            })
            .collect();
        Box::new(TransferObjectTestPayload {
            transfer_object: new_object,
            transfer_from: self.transfer_to,
            transfer_to: recipient.get_owner_address().unwrap(),
            gas: updated_gas,
            keypairs: self.keypairs.clone(),
        })
    }
    fn make_transaction(&self) -> VerifiedTransaction {
        let (gas_obj, _, _) = self
            .gas
            .iter()
            .find(|x| x.1.get_owner_address().unwrap() == self.transfer_from)
            .unwrap();
        make_transfer_object_transaction(
            self.transfer_object,
            *gas_obj,
            self.transfer_from,
            self.keypairs.get(&self.transfer_from).unwrap(),
            self.transfer_to,
        )
    }
    fn get_object_id(&self) -> ObjectID {
        self.transfer_object.0
    }
    fn get_workload_type(&self) -> WorkloadType {
        WorkloadType::TransferObject
    }
}

pub struct TransferObjectWorkload {
    pub transfer_keypairs: Arc<HashMap<SuiAddress, AccountKeyPair>>,
}

impl TransferObjectWorkload {
    pub fn new_boxed(num_accounts: u64) -> Box<dyn Workload<dyn Payload>> {
        // create several accounts to transfer object between
        let keypairs: Arc<HashMap<SuiAddress, AccountKeyPair>> =
            Arc::new((0..num_accounts).map(|_| get_key_pair()).collect());
        Box::new(TransferObjectWorkload {
            transfer_keypairs: keypairs,
        })
    }
    pub fn generate_coin_config_for_payloads(
        num_tokens: u64,
        num_transfer_accounts: u64,
        num_payloads: u64,
    ) -> Vec<GasCoinConfig> {
        let mut configs = vec![];

        // transfer tokens
        for _i in 0..num_tokens {
            let (address, keypair) = get_key_pair();
            configs.push(GasCoinConfig {
                amount: MAX_GAS_FOR_TESTING,
                address,
                keypair: Arc::new(keypair),
            });
        }

        // num_transfer_accounts
        for _i in 0..num_transfer_accounts {
            let (address, keypair) = get_key_pair();
            let cloned_keypair: Arc<AccountKeyPair> = Arc::new(keypair);
            for _j in 0..num_payloads {
                configs.push(GasCoinConfig {
                    amount: MAX_GAS_FOR_TESTING,
                    address,
                    keypair: cloned_keypair.clone(),
                });
            }
        }

        configs
    }
}

#[async_trait]
impl Workload<dyn Payload> for TransferObjectWorkload {
    async fn init(
        &mut self,
        _init_config: WorkloadInitConfig,
        _proxy: Arc<dyn ValidatorProxy + Sync + Send>,
    ) {
        return;
    }
    async fn make_test_payloads(
        &self,
        num_payloads: u64,
        payload_config: WorkloadPayloadConfig,
        _proxy: Arc<dyn ValidatorProxy + Sync + Send>,
    ) -> Vec<Box<dyn Payload>> {
        let gas_by_address: HashMap<SuiAddress, Vec<Gas>> = payload_config
            .transfer_object_payload_gas
            .into_iter()
            .group_by(|x| (*x).1.get_owner_address().expect("Failed to get address"))
            .into_iter()
            .map(|(address, group)| (address, group.collect()))
            .collect::<HashMap<SuiAddress, Vec<Gas>>>();

        // create as many gas objects as there are number of transfer objects times number of accounts
        info!("Creating enough gas to transfer objects..");
        let mut transfer_gas: Vec<Vec<Gas>> = vec![];
        for i in 0..num_payloads {
            let mut account_transfer_gas = vec![];
            for (address, _) in self.transfer_keypairs.iter() {
                account_transfer_gas.push(gas_by_address[address][i as usize].clone());
            }
            transfer_gas.push(account_transfer_gas);
        }
        let refs: Vec<(Vec<Gas>, Gas)> = transfer_gas
            .into_iter()
            .zip(payload_config.transfer_tokens.into_iter())
            .map(|(g, t)| (g, t.clone()))
            .collect();
        refs.into_iter()
            .map(|(g, t)| {
                let from = t.1;
                let to = g.iter().find(|x| x.1 != from).unwrap().1;
                Box::new(TransferObjectTestPayload {
                    transfer_object: t.0,
                    transfer_from: from.get_owner_address().unwrap(),
                    transfer_to: to.get_owner_address().unwrap(),
                    gas: g.clone(),
                    keypairs: self.transfer_keypairs.clone(),
                })
            })
            .map(|b| Box::<dyn Payload>::from(b))
            .collect()
    }
    fn get_workload_type(&self) -> WorkloadType {
        WorkloadType::TransferObject
    }
}
