// This file is part of Substrate.

// Copyright (C) 2021-2023 Parity Technologies (UK) Ltd.
// SPDX-License-Identifier: GPL-3.0-or-later WITH Classpath-exception-2.0

// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with this program. If not, see <https://www.gnu.org/licenses/>.

//! # MMR offchain gadget
//!
//! The MMR offchain gadget is run alongside `pallet-mmr` to assist it with offchain
//! canonicalization of finalized MMR leaves and nodes.
//! The gadget should only be run on nodes that have Indexing API enabled (otherwise
//! `pallet-mmr` cannot write to offchain and this gadget has nothing to do).
//!
//! The runtime `pallet-mmr` creates one new MMR leaf per block and all inner MMR parent nodes
//! generated by the MMR when adding said leaf. MMR nodes are stored both in:
//! - on-chain storage - hashes only; not full leaf content;
//! - off-chain storage - via Indexing API, full leaf content (and all internal nodes as well) is
//!   saved to the Off-chain DB using a key derived from `parent_hash` and node index in MMR. The
//!   `parent_hash` is also used within the key to avoid conflicts and overwrites on forks (leaf
//!   data is only allowed to reference data coming from parent block).
//!
//! This gadget is driven by block finality and in responsible for pruning stale forks from
//! offchain db, and moving finalized forks under a "canonical" key based solely on node `pos`
//! in the MMR.

#![warn(missing_docs)]

mod aux_schema;
mod offchain_mmr;
#[cfg(test)]
pub mod test_utils;

use crate::offchain_mmr::OffchainMmr;
use beefy_primitives::MmrRootHash;
use futures::StreamExt;
use log::{debug, error, trace, warn};
use sc_client_api::{Backend, BlockchainEvents, FinalityNotification, FinalityNotifications};
use sc_offchain::OffchainDb;
use sp_api::ProvideRuntimeApi;
use sp_blockchain::{HeaderBackend, HeaderMetadata};
use sp_mmr_primitives::{utils, LeafIndex, MmrApi};
use sp_runtime::{
	generic::BlockId,
	traits::{Block, Header, NumberFor},
};
use std::{marker::PhantomData, sync::Arc};

/// Logging target for the mmr gadget.
pub const LOG_TARGET: &str = "mmr";

/// A convenience MMR client trait that defines all the type bounds a MMR client
/// has to satisfy and defines some helper methods.
pub trait MmrClient<B, BE>:
	BlockchainEvents<B> + HeaderBackend<B> + HeaderMetadata<B> + ProvideRuntimeApi<B>
where
	B: Block,
	BE: Backend<B>,
	Self::Api: MmrApi<B, MmrRootHash, NumberFor<B>>,
{
	/// Get the block number where the mmr pallet was added to the runtime.
	fn first_mmr_block_num(&self, notification: &FinalityNotification<B>) -> Option<NumberFor<B>> {
		let best_block = *notification.header.number();
		match self.runtime_api().mmr_leaf_count(&BlockId::number(best_block)) {
			Ok(Ok(mmr_leaf_count)) => {
				match utils::first_mmr_block_num::<B::Header>(best_block, mmr_leaf_count) {
					Ok(first_mmr_block) => {
						debug!(
							target: LOG_TARGET,
							"pallet-mmr detected at block {:?} with genesis at block {:?}",
							best_block,
							first_mmr_block
						);
						Some(first_mmr_block)
					},
					Err(e) => {
						error!(
							target: LOG_TARGET,
							"Error calculating the first mmr block: {:?}", e
						);
						None
					},
				}
			},
			_ => {
				trace!(
					target: LOG_TARGET,
					"pallet-mmr not detected at block {:?} ... (best finalized {:?})",
					best_block,
					notification.header.number()
				);
				None
			},
		}
	}
}

impl<B, BE, T> MmrClient<B, BE> for T
where
	B: Block,
	BE: Backend<B>,
	T: BlockchainEvents<B> + HeaderBackend<B> + HeaderMetadata<B> + ProvideRuntimeApi<B>,
	T::Api: MmrApi<B, MmrRootHash, NumberFor<B>>,
{
	// empty
}

struct OffchainMmrBuilder<B: Block, BE: Backend<B>, C> {
	backend: Arc<BE>,
	client: Arc<C>,
	offchain_db: OffchainDb<BE::OffchainStorage>,
	indexing_prefix: Vec<u8>,

	_phantom: PhantomData<B>,
}

impl<B, BE, C> OffchainMmrBuilder<B, BE, C>
where
	B: Block,
	BE: Backend<B>,
	C: MmrClient<B, BE>,
	C::Api: MmrApi<B, MmrRootHash, NumberFor<B>>,
{
	async fn try_build(
		self,
		finality_notifications: &mut FinalityNotifications<B>,
	) -> Option<OffchainMmr<B, BE, C>> {
		while let Some(notification) = finality_notifications.next().await {
			if let Some(first_mmr_block_num) = self.client.first_mmr_block_num(&notification) {
				let mut offchain_mmr = OffchainMmr::new(
					self.backend,
					self.client,
					self.offchain_db,
					self.indexing_prefix,
					first_mmr_block_num,
				)?;
				// We need to make sure all blocks leading up to current notification
				// have also been canonicalized.
				offchain_mmr.canonicalize_catch_up(&notification);
				// We have to canonicalize and prune the blocks in the finality
				// notification that lead to building the offchain-mmr as well.
				offchain_mmr.canonicalize_and_prune(notification);
				return Some(offchain_mmr)
			}
		}

		error!(
			target: LOG_TARGET,
			"Finality notifications stream closed unexpectedly. \
			Couldn't build the canonicalization engine",
		);
		None
	}
}

/// A MMR Gadget.
pub struct MmrGadget<B: Block, BE: Backend<B>, C> {
	finality_notifications: FinalityNotifications<B>,

	_phantom: PhantomData<(B, BE, C)>,
}

impl<B, BE, C> MmrGadget<B, BE, C>
where
	B: Block,
	<B::Header as Header>::Number: Into<LeafIndex>,
	BE: Backend<B>,
	C: MmrClient<B, BE>,
	C::Api: MmrApi<B, MmrRootHash, NumberFor<B>>,
{
	async fn run(mut self, builder: OffchainMmrBuilder<B, BE, C>) {
		let mut offchain_mmr = match builder.try_build(&mut self.finality_notifications).await {
			Some(offchain_mmr) => offchain_mmr,
			None => return,
		};

		while let Some(notification) = self.finality_notifications.next().await {
			offchain_mmr.canonicalize_and_prune(notification);
		}
	}

	/// Create and run the MMR gadget.
	pub async fn start(client: Arc<C>, backend: Arc<BE>, indexing_prefix: Vec<u8>) {
		let offchain_db = match backend.offchain_storage() {
			Some(offchain_storage) => OffchainDb::new(offchain_storage),
			None => {
				warn!(
					target: LOG_TARGET,
					"Can't spawn a MmrGadget for a node without offchain storage."
				);
				return
			},
		};

		let mmr_gadget = MmrGadget::<B, BE, C> {
			finality_notifications: client.finality_notification_stream(),

			_phantom: Default::default(),
		};
		mmr_gadget
			.run(OffchainMmrBuilder {
				backend,
				client,
				offchain_db,
				indexing_prefix,
				_phantom: Default::default(),
			})
			.await
	}
}

#[cfg(test)]
mod tests {
	use crate::test_utils::run_test_with_mmr_gadget;
	use sp_runtime::generic::BlockId;
	use std::time::Duration;

	#[test]
	fn mmr_first_block_is_computed_correctly() {
		// Check the case where the first block is also the first block with MMR.
		run_test_with_mmr_gadget(|client| async move {
			// G -> A1 -> A2
			//      |
			//      | -> first mmr block

			let a1 = client.import_block(&BlockId::Number(0), b"a1", Some(0)).await;
			let a2 = client.import_block(&BlockId::Hash(a1.hash()), b"a2", Some(1)).await;

			client.finalize_block(a1.hash(), Some(1));
			tokio::time::sleep(Duration::from_millis(200)).await;
			// expected finalized heads: a1
			client.assert_canonicalized(&[&a1]);
			client.assert_not_pruned(&[&a2]);
		});

		// Check the case where the first block with MMR comes later.
		run_test_with_mmr_gadget(|client| async move {
			// G -> A1 -> A2 -> A3 -> A4 -> A5 -> A6
			//                        |
			//                        | -> first mmr block

			let a1 = client.import_block(&BlockId::Number(0), b"a1", None).await;
			let a2 = client.import_block(&BlockId::Hash(a1.hash()), b"a2", None).await;
			let a3 = client.import_block(&BlockId::Hash(a2.hash()), b"a3", None).await;
			let a4 = client.import_block(&BlockId::Hash(a3.hash()), b"a4", Some(0)).await;
			let a5 = client.import_block(&BlockId::Hash(a4.hash()), b"a5", Some(1)).await;
			let a6 = client.import_block(&BlockId::Hash(a5.hash()), b"a6", Some(2)).await;

			client.finalize_block(a5.hash(), Some(2));
			tokio::time::sleep(Duration::from_millis(200)).await;
			// expected finalized heads: a4, a5
			client.assert_canonicalized(&[&a4, &a5]);
			client.assert_not_pruned(&[&a6]);
		});
	}

	#[test]
	fn does_not_panic_on_invalid_num_mmr_blocks() {
		run_test_with_mmr_gadget(|client| async move {
			// G -> A1
			//      |
			//      | -> first mmr block

			let a1 = client.import_block(&BlockId::Number(0), b"a1", Some(0)).await;

			// Simulate the case where the runtime says that there are 2 mmr_blocks when in fact
			// there is only 1.
			client.finalize_block(a1.hash(), Some(2));
			tokio::time::sleep(Duration::from_millis(200)).await;
			// expected finalized heads: -
			client.assert_not_canonicalized(&[&a1]);
		});
	}
}
