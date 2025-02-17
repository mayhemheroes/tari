// Copyright 2021. The Tari Project
//
// Redistribution and use in source and binary forms, with or without modification, are permitted provided that the
// following conditions are met:
//
// 1. Redistributions of source code must retain the above copyright notice, this list of conditions and the following
// disclaimer.
//
// 2. Redistributions in binary form must reproduce the above copyright notice, this list of conditions and the
// following disclaimer in the documentation and/or other materials provided with the distribution.
//
// 3. Neither the name of the copyright holder nor the names of its contributors may be used to endorse or promote
// products derived from this software without specific prior written permission.
//
// THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES,
// INCLUDING, BUT NOT LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
// DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL,
// SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR
// SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY,
// WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE
// USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.

use serde::{Deserialize, Serialize};
use tari_common_types::types::BlockHash;

use crate::chain_storage::PrunedOutput;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UtxoMinedInfo {
    pub output: PrunedOutput,
    pub mmr_position: u32,
    pub mined_height: u64,
    pub header_hash: BlockHash,
    pub mined_timestamp: u64,
}

#[cfg(test)]
mod test {
    use tari_common_types::types::FixedHash;

    use super::*;

    impl UtxoMinedInfo {
        pub fn sample() -> Self {
            Self {
                output: PrunedOutput::sample(),
                mmr_position: 0,
                mined_height: 0,
                header_hash: FixedHash::zero(),
                mined_timestamp: 0,
            }
        }
    }

    #[test]
    fn coverage_utxo_mined_info() {
        let obj = UtxoMinedInfo::sample();
        drop(obj.clone());
        format!("{:?}", obj);
    }
}
