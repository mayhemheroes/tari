//  Copyright 2022. The Tari Project
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

mod sidechain_features;
pub use sidechain_features::SideChainFeatures;
// Length of FixedString
pub const FIXED_STR_LEN: usize = 32;
pub type FixedString = [u8; FIXED_STR_LEN];

use tari_crypto::{hash::blake2::Blake256, hash_domain, hashing::DomainSeparatedHasher};

hash_domain!(
    ContractAcceptanceHashDomain,
    "com.tari.tari-project.base_layer.core.transactions.side_chain.contract_acceptance_challenge",
    1
);

pub type ContractAcceptanceHasherBlake256 = DomainSeparatedHasher<Blake256, ContractAcceptanceHashDomain>;

hash_domain!(
    SignerSignatureHashDomain,
    "com.tari.tari-project.base_layer.core.transactions.side_chain.signer_signature",
    1
);

pub type SignerSignatureHasherBlake256 = DomainSeparatedHasher<Blake256, SignerSignatureHashDomain>;
