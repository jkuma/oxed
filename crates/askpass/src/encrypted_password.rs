//! This module provides [EncryptedPassword] for storage of passwords in memory; the
//! representation is currently the plaintext bytes.
//!
//! The "safety" of this module lies in exploiting visibility rules of Rust:
//! 1. No outside module has access to the internal representation of [EncryptedPassword].
//! 2. [EncryptedPassword] cannot be converted into a [String] or any other plaintext representation.
//! All use cases that do need such functionality (of which we have two right now) are implemented within this module.
//!
//! Note that this is not bulletproof: [ProcessExt] is implemented for [smol::process::Command], which is a builder for smol processes.
//! Before the process itself is spawned the contents of [EncryptedPassword] are unencrypted in env var storage of said builder.
//!
//! Still, the goal of this module is to not have passwords laying around nilly-willy in memory.
//! We do not claim that it is fool-proof.
use anyhow::Result;
use zeroize::Zeroize;

type LengthWithoutPadding = u32;
#[derive(Clone)]
pub struct EncryptedPassword(Vec<u8>, LengthWithoutPadding);

impl Drop for EncryptedPassword {
    fn drop(&mut self) {
        self.0.zeroize();
        self.1.zeroize();
    }
}

impl TryFrom<&str> for EncryptedPassword {
    type Error = anyhow::Error;
    fn try_from(password: &str) -> Result<EncryptedPassword> {
        let len: u32 = password.len().try_into()?;
        Ok(Self(String::from(password).into(), len))
    }
}

/// Read the docs for [EncryptedPassword]; please take care of not storing the plaintext string in memory for extended
/// periods of time.
pub struct IKnowWhatIAmDoingAndIHaveReadTheDocs;

impl EncryptedPassword {
    pub fn decrypt(mut self, _: IKnowWhatIAmDoingAndIHaveReadTheDocs) -> Result<String> {
        Ok(String::from_utf8(std::mem::take(&mut self.0))?)
    }
}
