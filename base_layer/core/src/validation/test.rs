//  Copyright 2020, The Tari Project
//
//  Redistribution and use in source and binary forms, with or without modification, are permitted provided that the
//  following conditions are met:
//
//  1. Redistributions of source code must retain the above copyright notice, this list of conditions and the following
//  disclaimer.
//
//  2. Redistributions in binary form must reproduce the above copyright notice, this list of conditions and the
//  following disclaimer in the documentation and/or other materials provided with the distribution.
//
//  3. Neither the name of the copyright holder nor the names of its contributors may be used to endorse or promote
//  products derived from this software without specific prior written permission.
//
//  THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES,
//  INCLUDING, BUT NOT LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
//  DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL,
//  SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR
//  SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY,
//  WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE
//  USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.

use std::sync::Arc;

use tari_common::configuration::Network;
use tari_common_types::types::Commitment;
use tari_crypto::commitment::HomomorphicCommitment;
use tari_script::script;
use tari_test_utils::unpack_enum;

use crate::{
    blocks::{BlockHeader, BlockHeaderAccumulatedData, ChainBlock, ChainHeader},
    chain_storage::DbTransaction,
    consensus::{ConsensusConstantsBuilder, ConsensusManager, ConsensusManagerBuilder},
    covenants::Covenant,
    proof_of_work::AchievedTargetDifficulty,
    test_helpers::{blockchain::create_store_with_consensus, create_chain_header},
    transactions::{
        tari_amount::{uT, MicroTari},
        test_helpers::{create_random_signature_from_s_key, create_utxo},
        transaction_components::{KernelBuilder, KernelFeatures, OutputFeatures, TransactionKernel},
        CryptoFactories,
    },
    tx,
    validation::{
        header_iter::HeaderIter,
        header_validator::HeaderValidator,
        transaction_validators::TxInternalConsistencyValidator,
        ChainBalanceValidator,
        DifficultyCalculator,
        FinalHorizonStateValidation,
        HeaderValidation,
        MempoolTransactionValidation,
        ValidationError,
    },
};

mod header_validators {
    use super::*;

    #[test]
    fn header_iter_empty_and_invalid_height() {
        let consensus_manager = ConsensusManager::builder(Network::LocalNet).build();
        let genesis = consensus_manager.get_genesis_block();
        let db = create_store_with_consensus(consensus_manager);

        let iter = HeaderIter::new(&db, 0, 10);
        let headers = iter.map(Result::unwrap).collect::<Vec<_>>();
        assert_eq!(headers.len(), 1);

        assert_eq!(genesis.header(), &headers[0]);

        // Invalid header height
        let iter = HeaderIter::new(&db, 1, 10);
        let headers = iter.collect::<Result<Vec<_>, _>>().unwrap();
        assert_eq!(headers.len(), 1);
    }

    #[test]
    fn header_iter_fetch_in_chunks() {
        let consensus_manager = ConsensusManagerBuilder::new(Network::LocalNet).build();
        let db = create_store_with_consensus(consensus_manager.clone());
        let headers = (1..=15).fold(vec![db.fetch_chain_header(0).unwrap()], |mut acc, i| {
            let prev = acc.last().unwrap();
            let mut header = BlockHeader::new(0);
            header.height = i;
            header.prev_hash = *prev.hash();
            // These have to be unique
            header.kernel_mmr_size = 2 + i;
            header.output_mmr_size = 4001 + i;

            let chain_header = create_chain_header(header, prev.accumulated_data());
            acc.push(chain_header);
            acc
        });
        db.insert_valid_headers(headers.into_iter().skip(1).collect()).unwrap();

        let iter = HeaderIter::new(&db, 11, 3);
        let headers = iter.map(Result::unwrap).collect::<Vec<_>>();
        assert_eq!(headers.len(), 12);
        let genesis = consensus_manager.get_genesis_block();
        assert_eq!(genesis.header(), &headers[0]);

        (1..=11).for_each(|i| {
            assert_eq!(headers[i].height, i as u64);
        })
    }

    #[test]
    fn it_validates_that_version_is_in_range() {
        let consensus_manager = ConsensusManagerBuilder::new(Network::LocalNet).build();
        let db = create_store_with_consensus(consensus_manager.clone());

        let genesis = db.fetch_chain_header(0).unwrap();

        let mut header = BlockHeader::from_previous(genesis.header());
        header.version = u16::MAX;

        let validator = HeaderValidator::new(consensus_manager.clone());

        let difficulty_calculator = DifficultyCalculator::new(consensus_manager, Default::default());
        let err = validator
            .validate(&*db.db_read_access().unwrap(), &header, &difficulty_calculator)
            .unwrap_err();
        assert!(matches!(err, ValidationError::InvalidBlockchainVersion {
            version: u16::MAX
        }));
    }
}

#[test]
#[allow(clippy::too_many_lines)]
fn chain_balance_validation() {
    let factories = CryptoFactories::default();
    let consensus_manager = ConsensusManagerBuilder::new(Network::Esmeralda).build();
    let genesis = consensus_manager.get_genesis_block();
    let faucet_value = 5000 * uT;
    let (faucet_utxo, faucet_key, _) = create_utxo(
        faucet_value,
        &factories,
        &OutputFeatures::default(),
        &script!(Nop),
        &Covenant::default(),
        MicroTari::zero(),
    );
    let (pk, sig) = create_random_signature_from_s_key(faucet_key, 0.into(), 0, KernelFeatures::empty());
    let excess = Commitment::from_public_key(&pk);
    let kernel =
        TransactionKernel::new_current_version(KernelFeatures::empty(), MicroTari::from(0), 0, excess, sig, None);
    let mut gen_block = genesis.block().clone();
    gen_block.body.add_output(faucet_utxo);
    gen_block.body.add_kernels(&mut vec![kernel]);
    let mut utxo_sum = HomomorphicCommitment::default();
    let mut kernel_sum = HomomorphicCommitment::default();
    let burned_sum = HomomorphicCommitment::default();
    for output in gen_block.body.outputs() {
        utxo_sum = &output.commitment + &utxo_sum;
    }
    for kernel in gen_block.body.kernels() {
        kernel_sum = &kernel.excess + &kernel_sum;
    }
    let genesis = ChainBlock::try_construct(Arc::new(gen_block), genesis.accumulated_data().clone()).unwrap();
    let total_faucet = faucet_value + consensus_manager.consensus_constants(0).faucet_value();
    let constants = ConsensusConstantsBuilder::new(Network::LocalNet)
        .with_consensus_constants(consensus_manager.consensus_constants(0).clone())
        .with_faucet_value(total_faucet)
        .build();
    // Create a LocalNet consensus manager that uses rincewind consensus constants and has a custom rincewind genesis
    // block that contains an extra faucet utxo
    let consensus_manager = ConsensusManagerBuilder::new(Network::LocalNet)
        .with_block(genesis.clone())
        .add_consensus_constants(constants)
        .build();

    let db = create_store_with_consensus(consensus_manager.clone());

    let validator = ChainBalanceValidator::new(consensus_manager.clone(), factories.clone());
    // Validate the genesis state
    validator
        .validate(&*db.db_read_access().unwrap(), 0, &utxo_sum, &kernel_sum, &burned_sum)
        .unwrap();

    //---------------------------------- Add a new coinbase and header --------------------------------------------//
    let mut txn = DbTransaction::new();
    let coinbase_value = consensus_manager.get_block_reward_at(1);
    let (coinbase, coinbase_key, _) = create_utxo(
        coinbase_value,
        &factories,
        &OutputFeatures::create_coinbase(1),
        &script!(Nop),
        &Covenant::default(),
        MicroTari::zero(),
    );
    // let _coinbase_hash = coinbase.hash();
    let (pk, sig) = create_random_signature_from_s_key(coinbase_key, 0.into(), 0, KernelFeatures::create_coinbase());
    let excess = Commitment::from_public_key(&pk);
    let kernel = KernelBuilder::new()
        .with_signature(&sig)
        .with_excess(&excess)
        .with_features(KernelFeatures::COINBASE_KERNEL)
        .build()
        .unwrap();

    let mut header1 = BlockHeader::from_previous(genesis.header());
    header1.kernel_mmr_size += 1;
    header1.output_mmr_size += 1;
    let achieved_difficulty = AchievedTargetDifficulty::try_construct(
        genesis.header().pow_algo(),
        genesis.accumulated_data().target_difficulty,
        genesis.accumulated_data().achieved_difficulty,
    )
    .unwrap();
    let accumulated_data = BlockHeaderAccumulatedData::builder(genesis.accumulated_data())
        .with_hash(header1.hash())
        .with_achieved_target_difficulty(achieved_difficulty)
        .with_total_kernel_offset(header1.total_kernel_offset.clone())
        .build()
        .unwrap();
    let header1 = ChainHeader::try_construct(header1, accumulated_data).unwrap();
    txn.insert_chain_header(header1.clone());

    let mut mmr_position = 4;
    let mut mmr_leaf_index = 4;

    txn.insert_kernel(kernel.clone(), *header1.hash(), mmr_position);
    txn.insert_utxo(coinbase.clone(), *header1.hash(), 1, mmr_leaf_index, 0);

    db.commit(txn).unwrap();
    utxo_sum = &coinbase.commitment + &utxo_sum;
    kernel_sum = &kernel.excess + &kernel_sum;
    validator
        .validate(&*db.db_read_access().unwrap(), 1, &utxo_sum, &kernel_sum, &burned_sum)
        .unwrap();

    //---------------------------------- Try to inflate --------------------------------------------//
    let mut txn = DbTransaction::new();

    let v = consensus_manager.get_block_reward_at(2) + uT;
    let (coinbase, key, _) = create_utxo(
        v,
        &factories,
        &OutputFeatures::create_coinbase(1),
        &script!(Nop),
        &Covenant::default(),
        MicroTari::zero(),
    );
    let (pk, sig) = create_random_signature_from_s_key(key, 0.into(), 0, KernelFeatures::create_coinbase());
    let excess = Commitment::from_public_key(&pk);
    let kernel = KernelBuilder::new()
        .with_signature(&sig)
        .with_excess(&excess)
        .with_features(KernelFeatures::COINBASE_KERNEL)
        .build()
        .unwrap();

    let mut header2 = BlockHeader::from_previous(header1.header());
    header2.kernel_mmr_size += 1;
    header2.output_mmr_size += 1;
    let achieved_difficulty = AchievedTargetDifficulty::try_construct(
        genesis.header().pow_algo(),
        genesis.accumulated_data().target_difficulty,
        genesis.accumulated_data().achieved_difficulty,
    )
    .unwrap();
    let accumulated_data = BlockHeaderAccumulatedData::builder(genesis.accumulated_data())
        .with_hash(header2.hash())
        .with_achieved_target_difficulty(achieved_difficulty)
        .with_total_kernel_offset(header2.total_kernel_offset.clone())
        .build()
        .unwrap();
    let header2 = ChainHeader::try_construct(header2, accumulated_data).unwrap();
    txn.insert_chain_header(header2.clone());
    utxo_sum = &coinbase.commitment + &utxo_sum;
    kernel_sum = &kernel.excess + &kernel_sum;
    mmr_leaf_index += 1;
    txn.insert_utxo(coinbase, *header2.hash(), 2, mmr_leaf_index, 0);
    mmr_position += 1;
    txn.insert_kernel(kernel, *header2.hash(), mmr_position);

    db.commit(txn).unwrap();

    validator
        .validate(&*db.db_read_access().unwrap(), 2, &utxo_sum, &kernel_sum, &burned_sum)
        .unwrap_err();
}

#[test]
#[allow(clippy::too_many_lines)]
fn chain_balance_validation_burned() {
    let factories = CryptoFactories::default();
    let consensus_manager = ConsensusManagerBuilder::new(Network::Esmeralda).build();
    let genesis = consensus_manager.get_genesis_block();
    let faucet_value = 5000 * uT;
    let (faucet_utxo, faucet_key, _) = create_utxo(
        faucet_value,
        &factories,
        &OutputFeatures::default(),
        &script!(Nop),
        &Covenant::default(),
        MicroTari::zero(),
    );
    let (pk, sig) = create_random_signature_from_s_key(faucet_key, 0.into(), 0, KernelFeatures::empty());
    let excess = Commitment::from_public_key(&pk);
    let kernel =
        TransactionKernel::new_current_version(KernelFeatures::empty(), MicroTari::from(0), 0, excess, sig, None);
    let mut gen_block = genesis.block().clone();
    gen_block.body.add_output(faucet_utxo);
    gen_block.body.add_kernels(&mut vec![kernel]);
    let mut utxo_sum = HomomorphicCommitment::default();
    let mut kernel_sum = HomomorphicCommitment::default();
    let mut burned_sum = HomomorphicCommitment::default();
    for output in gen_block.body.outputs() {
        utxo_sum = &output.commitment + &utxo_sum;
    }
    for kernel in gen_block.body.kernels() {
        kernel_sum = &kernel.excess + &kernel_sum;
    }
    let genesis = ChainBlock::try_construct(Arc::new(gen_block), genesis.accumulated_data().clone()).unwrap();
    let total_faucet = faucet_value + consensus_manager.consensus_constants(0).faucet_value();
    let constants = ConsensusConstantsBuilder::new(Network::LocalNet)
        .with_consensus_constants(consensus_manager.consensus_constants(0).clone())
        .with_faucet_value(total_faucet)
        .build();
    // Create a LocalNet consensus manager that uses rincewind consensus constants and has a custom rincewind genesis
    // block that contains an extra faucet utxo
    let consensus_manager = ConsensusManagerBuilder::new(Network::LocalNet)
        .with_block(genesis.clone())
        .add_consensus_constants(constants)
        .build();

    let db = create_store_with_consensus(consensus_manager.clone());

    let validator = ChainBalanceValidator::new(consensus_manager.clone(), factories.clone());
    // Validate the genesis state
    validator
        .validate(&*db.db_read_access().unwrap(), 0, &utxo_sum, &kernel_sum, &burned_sum)
        .unwrap();

    //---------------------------------- Add block (coinbase + burned) --------------------------------------------//
    let mut txn = DbTransaction::new();
    let coinbase_value = consensus_manager.get_block_reward_at(1) - MicroTari::from(100);
    let (coinbase, coinbase_key, _) = create_utxo(
        coinbase_value,
        &factories,
        &OutputFeatures::create_coinbase(1),
        &script!(Nop),
        &Covenant::default(),
        MicroTari::zero(),
    );
    let (pk, sig) = create_random_signature_from_s_key(coinbase_key, 0.into(), 0, KernelFeatures::create_coinbase());
    let excess = Commitment::from_public_key(&pk);
    let kernel = KernelBuilder::new()
        .with_signature(&sig)
        .with_excess(&excess)
        .with_features(KernelFeatures::COINBASE_KERNEL)
        .build()
        .unwrap();

    let (burned, burned_key, _) = create_utxo(
        100.into(),
        &factories,
        &OutputFeatures::create_burn_output(),
        &script!(Nop),
        &Covenant::default(),
        MicroTari::zero(),
    );

    let (pk2, sig2) = create_random_signature_from_s_key(burned_key, 0.into(), 0, KernelFeatures::create_burn());
    let excess2 = Commitment::from_public_key(&pk2);
    let kernel2 = KernelBuilder::new()
        .with_signature(&sig2)
        .with_excess(&excess2)
        .with_features(KernelFeatures::create_burn())
        .with_burn_commitment(Some(burned.commitment.clone()))
        .build()
        .unwrap();
    burned_sum = &burned_sum + kernel2.get_burn_commitment().unwrap();
    let mut header1 = BlockHeader::from_previous(genesis.header());
    header1.kernel_mmr_size += 2;
    header1.output_mmr_size += 2;
    let achieved_difficulty = AchievedTargetDifficulty::try_construct(
        genesis.header().pow_algo(),
        genesis.accumulated_data().target_difficulty,
        genesis.accumulated_data().achieved_difficulty,
    )
    .unwrap();
    let accumulated_data = BlockHeaderAccumulatedData::builder(genesis.accumulated_data())
        .with_hash(header1.hash())
        .with_achieved_target_difficulty(achieved_difficulty)
        .with_total_kernel_offset(header1.total_kernel_offset.clone())
        .build()
        .unwrap();
    let header1 = ChainHeader::try_construct(header1, accumulated_data).unwrap();
    txn.insert_chain_header(header1.clone());

    let mut mmr_position = 4;
    let mut mmr_leaf_index = 4;

    txn.insert_kernel(kernel.clone(), *header1.hash(), mmr_position);
    txn.insert_utxo(coinbase.clone(), *header1.hash(), 1, mmr_leaf_index, 0);

    mmr_position = 5;
    mmr_leaf_index = 5;

    txn.insert_kernel(kernel2.clone(), *header1.hash(), mmr_position);
    txn.insert_pruned_utxo(
        burned.hash(),
        burned.witness_hash(),
        *header1.hash(),
        header1.height(),
        mmr_leaf_index,
        0,
    );

    db.commit(txn).unwrap();
    utxo_sum = &coinbase.commitment + &utxo_sum;
    kernel_sum = &(&kernel.excess + &kernel_sum) + &kernel2.excess;
    validator
        .validate(&*db.db_read_access().unwrap(), 1, &utxo_sum, &kernel_sum, &burned_sum)
        .unwrap();
}

mod transaction_validator {
    use super::*;

    #[test]
    fn it_rejects_coinbase_outputs() {
        let consensus_manager = ConsensusManagerBuilder::new(Network::LocalNet).build();
        let db = create_store_with_consensus(consensus_manager);
        let factories = CryptoFactories::default();
        let validator = TxInternalConsistencyValidator::new(factories, true, db);
        let features = OutputFeatures::create_coinbase(0);
        let (tx, _, _) = tx!(MicroTari(100_000), fee: MicroTari(5), inputs: 1, outputs: 1, features: features);
        let err = validator.validate(&tx).unwrap_err();
        unpack_enum!(ValidationError::ErroneousCoinbaseOutput = err);
    }
}
