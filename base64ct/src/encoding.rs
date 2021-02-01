//! Base64 encodings

use crate::{
    errors::{Error, InvalidEncodingError, InvalidLengthError},
    variant::Variant,
};
use core::{ops::Range, str};

#[cfg(feature = "alloc")]
use alloc::{string::String, vec::Vec};

/// Padding character
const PAD: u8 = b'=';

/// Base64 encoding
pub trait Encoding {
    /// Decode a Base64 string into the provided destination buffer.
    fn decode(src: impl AsRef<[u8]>, dst: &mut [u8]) -> Result<&[u8], Error>;

    /// Decode a Base64 string in-place.
    fn decode_in_place(buf: &mut [u8]) -> Result<&[u8], InvalidEncodingError>;

    /// Decode a Base64 string into a byte vector.
    #[cfg(feature = "alloc")]
    #[cfg_attr(docsrs, doc(cfg(feature = "alloc")))]
    fn decode_vec(input: &str) -> Result<Vec<u8>, Error>;

    /// Encode the input byte slice as Base64.
    ///
    /// Writes the result into the provided destination slice, returning an
    /// ASCII-encoded Base64 string value.
    fn encode<'a>(src: &[u8], dst: &'a mut [u8]) -> Result<&'a str, InvalidLengthError>;

    /// Encode input byte slice into a [`String`] containing Base64.
    ///
    /// # Panics
    /// If `input` length is greater than `usize::MAX/4`.
    #[cfg(feature = "alloc")]
    #[cfg_attr(docsrs, doc(cfg(feature = "alloc")))]
    fn encode_string(input: &[u8]) -> String;

    /// Get the length of Base64 produced by encoding the given bytes.
    ///
    /// WARNING: this function will return `0` for lengths greater than `usize::MAX/4`!
    fn encoded_len(bytes: &[u8]) -> usize;
}

impl<T: Variant> Encoding for T {
    fn decode(src: impl AsRef<[u8]>, dst: &mut [u8]) -> Result<&[u8], Error> {
        let mut src = src.as_ref();

        let mut err = if T::PADDED {
            let (unpadded_len, e) = decode_padding(src)?;
            src = &src[..unpadded_len];
            e
        } else {
            0
        };

        let dlen = decoded_len(src.len());

        if dlen > dst.len() {
            return Err(Error::InvalidLength);
        }

        let dst = &mut dst[..dlen];

        let mut src_chunks = src.chunks_exact(4);
        let mut dst_chunks = dst.chunks_exact_mut(3);
        for (s, d) in (&mut src_chunks).zip(&mut dst_chunks) {
            err |= decode_3bytes(s, d, Self::decode_6bits);
        }
        let src_rem = src_chunks.remainder();
        let dst_rem = dst_chunks.into_remainder();

        err |= !(src_rem.is_empty() || src_rem.len() >= 2) as i16;
        let mut tmp_out = [0u8; 3];
        let mut tmp_in = [b'A'; 4];
        tmp_in[..src_rem.len()].copy_from_slice(src_rem);
        err |= decode_3bytes(&tmp_in, &mut tmp_out, Self::decode_6bits);
        dst_rem.copy_from_slice(&tmp_out[..dst_rem.len()]);

        if err == 0 {
            Ok(dst)
        } else {
            Err(Error::InvalidEncoding)
        }
    }

    /// Decode a standard Base64 string without padding in-place.
    fn decode_in_place(mut buf: &mut [u8]) -> Result<&[u8], InvalidEncodingError> {
        // TODO: eliminate unsafe code when compiler will be smart enough to
        // eliminate bound checks, see: https://github.com/rust-lang/rust/issues/80963
        let mut err = if T::PADDED {
            let (unpadded_len, e) = decode_padding(buf)?;
            buf = &mut buf[..unpadded_len];
            e
        } else {
            0
        };

        let dlen = decoded_len(buf.len());
        let full_chunks = buf.len() / 4;

        for chunk in 0..full_chunks {
            // SAFETY: `p3` and `p4` point inside `buf`, while they may overlap,
            // read and write are clearly separated from each other and done via
            // raw pointers.
            unsafe {
                debug_assert!(3 * chunk + 3 <= buf.len());
                debug_assert!(4 * chunk + 4 <= buf.len());

                let p3 = buf.as_mut_ptr().add(3 * chunk) as *mut [u8; 3];
                let p4 = buf.as_ptr().add(4 * chunk) as *const [u8; 4];

                let mut tmp_out = [0u8; 3];
                err |= decode_3bytes(&*p4, &mut tmp_out, Self::decode_6bits);
                *p3 = tmp_out;
            }
        }

        let src_rem_pos = 4 * full_chunks;
        let src_rem_len = buf.len() - src_rem_pos;
        let dst_rem_pos = 3 * full_chunks;
        let dst_rem_len = dlen - dst_rem_pos;

        err |= !(src_rem_len == 0 || src_rem_len >= 2) as i16;
        let mut tmp_in = [b'A'; 4];
        tmp_in[..src_rem_len].copy_from_slice(&buf[src_rem_pos..]);
        let mut tmp_out = [0u8; 3];

        err |= decode_3bytes(&tmp_in, &mut tmp_out, Self::decode_6bits);

        if err == 0 {
            // SAFETY: `dst_rem_len` is always smaller than 4, so we don't
            // read outside of `tmp_out`, write and the final slicing never go
            // outside of `buf`.
            unsafe {
                debug_assert!(dst_rem_pos + dst_rem_len <= buf.len());
                debug_assert!(dst_rem_len <= tmp_out.len());
                debug_assert!(dlen <= buf.len());

                core::ptr::copy_nonoverlapping(
                    tmp_out.as_ptr(),
                    buf.as_mut_ptr().add(dst_rem_pos),
                    dst_rem_len,
                );
                Ok(buf.get_unchecked(..dlen))
            }
        } else {
            Err(InvalidEncodingError)
        }
    }

    #[cfg(feature = "alloc")]
    fn decode_vec(input: &str) -> Result<Vec<u8>, Error> {
        let mut output = vec![0u8; decoded_len(input.len())];
        let len = Self::decode(input, &mut output)?.len();

        if len <= output.len() {
            output.truncate(len);
            Ok(output)
        } else {
            Err(Error::InvalidLength)
        }
    }

    fn encode<'a>(src: &[u8], dst: &'a mut [u8]) -> Result<&'a str, InvalidLengthError> {
        let elen = match encoded_len_inner(src.len(), T::PADDED) {
            Some(v) => v,
            None => return Err(InvalidLengthError),
        };

        if elen > dst.len() {
            return Err(InvalidLengthError);
        }

        let dst = &mut dst[..elen];

        let mut src_chunks = src.chunks_exact(3);
        let mut dst_chunks = dst.chunks_exact_mut(4);

        for (s, d) in (&mut src_chunks).zip(&mut dst_chunks) {
            encode_3bytes(s, d, Self::encode_6bits);
        }

        let src_rem = src_chunks.remainder();

        if T::PADDED {
            if let Some(dst_rem) = dst_chunks.next() {
                let mut tmp = [0u8; 3];
                tmp[..src_rem.len()].copy_from_slice(&src_rem);
                encode_3bytes(&tmp, dst_rem, Self::encode_6bits);

                let flag = src_rem.len() == 1;
                let mask = (flag as u8).wrapping_sub(1);
                dst_rem[2] = (dst_rem[2] & mask) | (PAD & !mask);
                dst_rem[3] = PAD;
            }
        } else {
            let dst_rem = dst_chunks.into_remainder();

            let mut tmp_in = [0u8; 3];
            let mut tmp_out = [0u8; 4];
            tmp_in[..src_rem.len()].copy_from_slice(src_rem);
            encode_3bytes(&tmp_in, &mut tmp_out, Self::encode_6bits);
            dst_rem.copy_from_slice(&tmp_out[..dst_rem.len()]);
        }

        debug_assert!(str::from_utf8(dst).is_ok());

        // SAFETY: values written by `encode_3bytes` are valid one-byte UTF-8 chars
        Ok(unsafe { str::from_utf8_unchecked(dst) })
    }

    #[cfg(feature = "alloc")]
    fn encode_string(input: &[u8]) -> String {
        let elen = encoded_len_inner(input.len(), T::PADDED).expect("input is too big");
        let mut dst = vec![0u8; elen];
        let res = Self::encode(input, &mut dst).expect("encoding error");

        debug_assert_eq!(elen, res.len());
        debug_assert!(str::from_utf8(&dst).is_ok());

        // SAFETY: `dst` is fully written and contains only valid one-byte UTF-8 chars
        unsafe { String::from_utf8_unchecked(dst) }
    }

    fn encoded_len(bytes: &[u8]) -> usize {
        // TODO: replace with `unwrap_or` on stabilization
        match encoded_len_inner(bytes.len(), T::PADDED) {
            Some(v) => v,
            None => 0,
        }
    }
}

/// Get the length of the output from decoding the provided *unpadded*
/// Base64-encoded input (use [`unpadded_len_ct`] to compute this value for
/// a padded input)
///
/// Note that this function does not fully validate the Base64 is well-formed
/// and may return incorrect results for malformed Base64.
#[inline(always)]
fn decoded_len(input_len: usize) -> usize {
    // overflow-proof computation of `(3*n)/4`
    let k = input_len / 4;
    let l = input_len - 4 * k;
    3 * k + (3 * l) / 4
}

/// Decode 3 bytes of a Base64 message.
#[inline(always)]
fn decode_3bytes<F>(src: &[u8], dst: &mut [u8], decode_6bits: F) -> i16
where
    F: Fn(u8) -> i16 + Copy,
{
    debug_assert_eq!(src.len(), 4);
    debug_assert!(dst.len() >= 3, "dst too short: {}", dst.len());

    let c0 = decode_6bits(src[0]);
    let c1 = decode_6bits(src[1]);
    let c2 = decode_6bits(src[2]);
    let c3 = decode_6bits(src[3]);

    dst[0] = ((c0 << 2) | (c1 >> 4)) as u8;
    dst[1] = ((c1 << 4) | (c2 >> 2)) as u8;
    dst[2] = ((c2 << 6) | c3) as u8;

    ((c0 | c1 | c2 | c3) >> 8) & 1
}

/// Validate padding is well-formed and compute unpadded length.
///
/// Returns length-related errors eagerly as a [`Result`], and data-dependent
/// errors (i.e. malformed padding bytes) as `i16` to be combined with other
/// encoding-related errors prior to branching.
#[inline(always)]
fn decode_padding(input: &[u8]) -> Result<(usize, i16), InvalidEncodingError> {
    if input.len() % 4 != 0 {
        return Err(InvalidEncodingError);
    }

    let unpadded_len = match *input {
        [.., b0, b1] => {
            let pad_len = match_eq_ct(b0, PAD, 1) + match_eq_ct(b1, PAD, 1);
            input.len() - pad_len as usize
        }
        _ => input.len(),
    };

    let padding_len = input.len() - unpadded_len;

    let err = match *input {
        [.., b0] if padding_len == 1 => match_eq_ct(b0, PAD, 1) ^ 1,
        [.., b0, b1] if padding_len == 2 => (match_eq_ct(b0, PAD, 1) & match_eq_ct(b1, PAD, 1)) ^ 1,
        _ => {
            if padding_len == 0 {
                0
            } else {
                return Err(InvalidEncodingError);
            }
        }
    };

    Ok((unpadded_len, err))
}

#[inline(always)]
fn encode_3bytes<F>(src: &[u8], dst: &mut [u8], encode_6bits: F)
where
    F: Fn(i16) -> u8 + Copy,
{
    debug_assert_eq!(src.len(), 3);
    debug_assert!(dst.len() >= 4, "dst too short: {}", dst.len());

    let b0 = src[0] as i16;
    let b1 = src[1] as i16;
    let b2 = src[2] as i16;

    dst[0] = encode_6bits(b0 >> 2);
    dst[1] = encode_6bits(((b0 << 4) | (b1 >> 4)) & 63);
    dst[2] = encode_6bits(((b1 << 2) | (b2 >> 6)) & 63);
    dst[3] = encode_6bits(b2 & 63);
}

#[inline(always)]
const fn encoded_len_inner(n: usize, padded: bool) -> Option<usize> {
    // TODO: replace with `checked_mul` and `map` on stabilization
    if n > usize::MAX / 4 {
        return None;
    }

    let q = 4 * n;

    if padded {
        Some(((q / 3) + 3) & !3)
    } else {
        Some((q / 3) + (q % 3 != 0) as usize)
    }
}

/// Match a a byte equals a specified value.
#[inline(always)]
pub(crate) fn match_eq_ct(input: u8, expected: u8, ret_on_match: i16) -> i16 {
    match_range_ct(input, expected..expected, ret_on_match)
}

/// Match that the given input is greater than the provided threshold.
#[inline(always)]
pub(crate) fn match_gt_ct(input: i16, threshold: u8, ret_on_match: i16) -> i16 {
    ((threshold as i16 - input) >> 8) & ret_on_match
}

/// Match that a byte falls within a provided range.
#[inline(always)]
pub(crate) fn match_range_ct(input: u8, range: Range<u8>, ret_on_match: i16) -> i16 {
    // Compute exclusive range from inclusive one
    let start = range.start as i16 - 1;
    let end = range.end as i16 + 1;

    (((start - input as i16) & (input as i16 - end)) >> 8) & ret_on_match
}