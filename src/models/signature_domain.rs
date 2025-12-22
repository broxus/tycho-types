//! Signature domain models.

use std::borrow::Cow;

#[cfg(feature = "abi")]
use ed25519_dalek::Signer;
use tl_proto::{TlRead, TlWrite};

/// Signature domain variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, TlRead, TlWrite)]
#[tl(
    boxed,
    scheme_inline = r#"
    signature_domain.l2#71b34ee1 global_id:int = SignatureDomain;
    signature_domain.empty#e1d571b = SignatureDomain;
    "#
)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(tag = "type", content = "id"))]
pub enum SignatureDomain {
    /// Special variant to NOT add any prefix for the verified data.
    /// Can be used to verify mainnet signatures from L2 networks.
    #[tl(id = "signature_domain.empty")]
    Empty,
    /// Non-empty variant. Hash of its TL representation
    /// is used as a prefix for the verified data.
    #[tl(id = "signature_domain.l2")]
    L2 {
        /// Global id of the network.
        global_id: i32,
    },
}

impl SignatureDomain {
    /// Signs arbitrary data using the key and optional signature id.
    #[cfg(feature = "abi")]
    pub fn sign(&self, key: &ed25519_dalek::SigningKey, data: &[u8]) -> ed25519_dalek::Signature {
        let data = self.apply(data);
        key.sign(&data)
    }

    /// Prepares arbitrary data for signing.
    pub fn apply<'a>(&self, data: &'a [u8]) -> Cow<'a, [u8]> {
        if let Self::Empty = self {
            Cow::Borrowed(data)
        } else {
            let hash = tl_proto::hash(self);

            let mut result = Vec::with_capacity(32 + data.len());
            result.extend_from_slice(&hash);
            result.extend_from_slice(data);
            Cow::Owned(result)
        }
    }
}
