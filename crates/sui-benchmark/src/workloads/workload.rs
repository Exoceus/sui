// Copyright (c) Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

use async_trait::async_trait;
use std::sync::Arc;
use std::{collections::HashMap, fmt};

use sui_types::base_types::{ObjectID, ObjectRef};

use sui_types::messages::VerifiedTransaction;

use crate::util::Gas;
use rand::{prelude::*, rngs::OsRng};
use rand_distr::WeightedAliasIndex;
use sui_types::base_types::SuiAddress;
use sui_types::crypto::AccountKeyPair;

use crate::ValidatorProxy;

// This is the maximum gas we will transfer from primary coin into any gas coin
// for running the benchmark
pub const MAX_GAS_FOR_TESTING: u64 = 1_000_000_000;

pub trait Payload: Send + Sync {
    fn make_new_payload(
        self: Box<Self>,
        new_object: ObjectRef,
        new_gas: ObjectRef,
    ) -> Box<dyn Payload>;
    fn make_transaction(&self) -> VerifiedTransaction;
    fn get_object_id(&self) -> ObjectID;
    fn get_workload_type(&self) -> WorkloadType;
}

pub struct CombinationPayload {
    payloads: Vec<Box<dyn Payload>>,
    dist: WeightedAliasIndex<u32>,
    curr_index: usize,
    rng: OsRng,
}

impl Payload for CombinationPayload {
    fn make_new_payload(
        self: Box<Self>,
        new_object: ObjectRef,
        new_gas: ObjectRef,
    ) -> Box<dyn Payload> {
        let mut new_payloads = vec![];
        for (pos, e) in self.payloads.into_iter().enumerate() {
            if pos == self.curr_index {
                let updated = e.make_new_payload(new_object, new_gas);
                new_payloads.push(updated);
            } else {
                new_payloads.push(e);
            }
        }
        let mut rng = self.rng;
        let next_index = self.dist.sample(&mut rng);
        Box::new(CombinationPayload {
            payloads: new_payloads,
            dist: self.dist,
            curr_index: next_index,
            rng: self.rng,
        })
    }
    fn make_transaction(&self) -> VerifiedTransaction {
        let curr = self.payloads.get(self.curr_index).unwrap();
        curr.make_transaction()
    }
    fn get_object_id(&self) -> ObjectID {
        let curr = self.payloads.get(self.curr_index).unwrap();
        curr.get_object_id()
    }
    fn get_workload_type(&self) -> WorkloadType {
        self.payloads
            .get(self.curr_index)
            .unwrap()
            .get_workload_type()
    }
}

#[derive(Copy, Clone, Hash, PartialEq, Eq)]
pub enum WorkloadType {
    SharedCounter,
    TransferObject,
    Combination,
}

impl fmt::Display for WorkloadType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            WorkloadType::SharedCounter => write!(f, "shared_counter"),
            WorkloadType::TransferObject => write!(f, "transfer_object"),
            WorkloadType::Combination => write!(f, "combination"),
        }
    }
}

#[derive(Clone)]
pub struct GasCoinConfig {
    // amount of SUI to transfer to this gas coin
    pub amount: u64,
    // recipient of this gas coin
    pub address: SuiAddress,
    // recipient account key pair (useful for signing txns)
    pub keypair: Arc<AccountKeyPair>,
}

#[derive(Clone)]
pub struct WorkloadInitConfig {
    // Gas coins to initialize shared counter workload
    // This includes the coins to publish the package and create
    // shared counters
    pub shared_counter_init_gas: Vec<Gas>,
}

#[derive(Clone)]
pub struct WorkloadPayloadConfig {
    // Gas coins to be used as transfer tokens
    // These are the objects which get transferred
    // between different accounts during the course of benchmark
    pub transfer_tokens: Vec<Gas>,
    // Gas coins needed to run transfer transactions during
    // the course of benchmark
    pub transfer_object_payload_gas: Vec<Gas>,
    // Gas coins needed to run shared counter increment during
    // the course of benchmark
    pub shared_counter_payload_gas: Vec<Gas>,
}

#[async_trait]
pub trait Workload<T: Payload + ?Sized>: Send + Sync {
    async fn init(
        &mut self,
        init_config: WorkloadInitConfig,
        proxy: Arc<dyn ValidatorProxy + Sync + Send>,
    );
    async fn make_test_payloads(
        &self,
        num_payloads: u64,
        payload_config: WorkloadPayloadConfig,
        proxy: Arc<dyn ValidatorProxy + Sync + Send>,
    ) -> Vec<Box<T>>;
    fn get_workload_type(&self) -> WorkloadType;
}

type WeightAndPayload = (u32, Box<dyn Workload<dyn Payload>>);
pub struct CombinationWorkload {
    workloads: HashMap<WorkloadType, WeightAndPayload>,
}

#[async_trait]
impl Workload<dyn Payload> for CombinationWorkload {
    async fn init(
        &mut self,
        init_config: WorkloadInitConfig,
        proxy: Arc<dyn ValidatorProxy + Sync + Send>,
    ) {
        for (_, (_, workload)) in self.workloads.iter_mut() {
            workload.init(init_config.clone(), proxy.clone()).await;
        }
    }
    async fn make_test_payloads(
        &self,
        num_payloads: u64,
        payload_config: WorkloadPayloadConfig,
        proxy: Arc<dyn ValidatorProxy + Sync + Send>,
    ) -> Vec<Box<dyn Payload>> {
        let mut workloads: HashMap<WorkloadType, (u32, Vec<Box<dyn Payload>>)> = HashMap::new();
        for (workload_type, (weight, workload)) in self.workloads.iter() {
            let payloads: Vec<Box<dyn Payload>> = workload
                .make_test_payloads(num_payloads, payload_config.clone(), proxy.clone())
                .await;
            assert_eq!(payloads.len() as u64, num_payloads);
            workloads
                .entry(*workload_type)
                .or_insert_with(|| (*weight, payloads));
        }
        let mut res = vec![];
        for _i in 0..num_payloads {
            let mut all_payloads: Vec<Box<dyn Payload>> = vec![];
            let mut dist = vec![];
            for (_type, (weight, payloads)) in workloads.iter_mut() {
                all_payloads.push(payloads.pop().unwrap());
                dist.push(*weight);
            }
            res.push(Box::new(CombinationPayload {
                payloads: all_payloads,
                dist: WeightedAliasIndex::new(dist).unwrap(),
                curr_index: 0,
                rng: OsRng::default(),
            }));
        }
        res.into_iter()
            .map(|b| Box::<dyn Payload>::from(b))
            .collect()
    }
    fn get_workload_type(&self) -> WorkloadType {
        WorkloadType::Combination
    }
}

impl CombinationWorkload {
    pub fn new_boxed(
        workloads: HashMap<WorkloadType, WeightAndPayload>,
    ) -> Box<dyn Workload<dyn Payload>> {
        Box::new(CombinationWorkload { workloads })
    }
}

pub struct WorkloadInfo {
    pub target_qps: u64,
    pub num_workers: u64,
    pub max_in_flight_ops: u64,
    pub workload: Box<dyn Workload<dyn Payload>>,
    pub payload_config: WorkloadPayloadConfig,
}
