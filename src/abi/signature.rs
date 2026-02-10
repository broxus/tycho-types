use std::borrow::Cow;

use ed25519_dalek::Signer;

/// Signs arbitrary data using the key and optional signature id.
#[deprecated = "use `AutoSignatureContext::sign` instead"]
pub fn sign_with_signature_id(
    key: &ed25519_dalek::SigningKey,
    data: &[u8],
    signature_id: Option<i32>,
) -> ed25519_dalek::Signature {
    #[allow(deprecated)]
    let data = extend_signature_with_id(data, signature_id);
    key.sign(&data)
}

/// Prepares arbitrary data for signing.
#[deprecated = "use `AutoSignatureContext::apply` instead"]
pub fn extend_signature_with_id(data: &[u8], signature_id: Option<i32>) -> Cow<'_, [u8]> {
    match signature_id {
        Some(signature_id) => {
            let mut result = Vec::with_capacity(4 + data.len());
            result.extend_from_slice(&signature_id.to_be_bytes());
            result.extend_from_slice(data);
            Cow::Owned(result)
        }
        None => Cow::Borrowed(data),
    }
}
