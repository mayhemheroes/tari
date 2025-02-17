// Copyright 2020. The Tari Project
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

use std::convert::{TryFrom, TryInto};

use tari_core::transactions::transaction_components::{
    OutputFeatures,
    OutputFeaturesVersion,
    OutputType,
    SideChainFeatures,
};

use crate::tari_rpc as grpc;

impl TryFrom<grpc::OutputFeatures> for OutputFeatures {
    type Error = String;

    fn try_from(features: grpc::OutputFeatures) -> Result<Self, Self::Error> {
        let sidechain_features = features
            .sidechain_features
            .map(SideChainFeatures::try_from)
            .transpose()?;

        let output_type = features
            .output_type
            .try_into()
            .map_err(|_| "Invalid output type: overflow")?;

        Ok(OutputFeatures::new(
            OutputFeaturesVersion::try_from(
                u8::try_from(features.version).map_err(|_| "Invalid version: overflowed u8")?,
            )?,
            OutputType::from_byte(output_type).ok_or_else(|| "Invalid or unrecognised output type".to_string())?,
            features.maturity,
            features.metadata,
            sidechain_features,
        ))
    }
}

impl From<OutputFeatures> for grpc::OutputFeatures {
    fn from(features: OutputFeatures) -> Self {
        Self {
            version: features.version as u32,
            output_type: u32::from(features.output_type.as_byte()),
            maturity: features.maturity,
            metadata: features.metadata,
            sidechain_features: features.sidechain_features.map(|v| *v).map(Into::into),
        }
    }
}
