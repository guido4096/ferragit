// SPDX-FileCopyrightText: 2026 Guido <guido@ngjansen.be>
// SPDX-License-Identifier: GPL-2.0-only

//! Git's delta bytecode. Every read, copy and output growth is bounded before use.
use crate::{Error, Result};

fn invalid(message: &str) -> Error {
    Error::Invalid(format!("corrupt delta: {message}"))
}

pub(super) fn integer(bytes: &mut &[u8]) -> Result<u64> {
    let mut value = 0u64;
    let mut shift = 0;
    loop {
        let (&byte, rest) = bytes
            .split_first()
            .ok_or_else(|| invalid("truncated integer"))?;
        *bytes = rest;
        let chunk = u64::from(byte & 0x7f);
        if shift >= 64 || chunk > (u64::MAX >> shift) {
            return Err(invalid("integer overflow"));
        }
        value |= chunk << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
        shift += 7;
    }
}

pub(super) fn result_size(delta: &[u8], limit: u64) -> Result<usize> {
    let mut bytes = delta;
    integer(&mut bytes)?;
    let size = integer(&mut bytes)?;
    if size > limit {
        return Err(Error::Unsupported(
            "delta result exceeds object size limit".into(),
        ));
    }
    usize::try_from(size).map_err(|_| invalid("result size overflow"))
}

pub(super) fn apply(base: &[u8], delta: &[u8], limit: u64) -> Result<Vec<u8>> {
    let size = result_size(delta, limit)?;
    let mut bytes = delta;
    if integer(&mut bytes)? != base.len() as u64 {
        return Err(invalid("base size mismatch"));
    }
    integer(&mut bytes)?;
    let mut output = Vec::with_capacity(size);
    while let Some((&instruction, rest)) = bytes.split_first() {
        bytes = rest;
        if instruction == 0 {
            return Err(invalid("reserved opcode"));
        }
        if instruction & 0x80 == 0 {
            let length = usize::from(instruction);
            if length > bytes.len() || length > size - output.len() {
                return Err(invalid("literal exceeds input or result size"));
            }
            output.extend_from_slice(&bytes[..length]);
            bytes = &bytes[length..];
        } else {
            let mut offset = 0usize;
            let mut length = 0usize;
            for bit in 0..7 {
                if instruction & (1 << bit) == 0 {
                    continue;
                }
                let (&byte, rest) = bytes
                    .split_first()
                    .ok_or_else(|| invalid("truncated copy instruction"))?;
                bytes = rest;
                if bit < 4 {
                    offset |= usize::from(byte) << (bit * 8);
                } else {
                    length |= usize::from(byte) << ((bit - 4) * 8);
                }
            }
            if length == 0 {
                length = 0x10000;
            }
            if offset > base.len() || length > base.len() - offset || length > size - output.len() {
                return Err(invalid("copy exceeds base or result size"));
            }
            output.extend_from_slice(&base[offset..offset + length]);
        }
    }
    if output.len() != size {
        return Err(invalid("result size mismatch"));
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn copies_and_inserts_and_default_copy_length() {
        assert_eq!(
            apply(b"hello", &[5, 6, 0x90, 5, 1, b'!'], 100).unwrap(),
            b"hello!"
        );
        assert_eq!(apply(b"hello", &[5, 3, 0x91, 1, 3], 100).unwrap(), b"ell");
        let base = vec![42; 65536];
        assert_eq!(
            apply(&base, &[0x80, 0x80, 4, 0x80, 0x80, 4, 0x80], 65536).unwrap(),
            base
        );
    }
    #[test]
    fn malformed_instructions_and_bombs_fail_closed() {
        for delta in [
            vec![],
            vec![5],
            vec![4, 0],
            vec![5, 1, 0],
            vec![5, 1, 2, b'a'],
            vec![5, 1, 0x91, 10, 1],
            vec![5, 1, 0x90, 2],
            vec![5, 1, 0x91],
            vec![5, 2, 1, b'a'],
            vec![5, 1, 1, b'a', 1, b'b'],
            vec![0x80; 20],
        ] {
            assert!(apply(b"hello", &delta, 100).is_err(), "accepted {delta:?}");
        }
        assert!(apply(b"", &[0, 0x80, 0x80, 0x80, 1], 1024).is_err());
    }
}
