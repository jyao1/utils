//! DER encoder.

use crate::{
    asn1::sequence, BitString, Encodable, ErrorKind, GeneralizedTime, Header, Ia5String, Length,
    Null, OctetString, PrintableString, Result, Tag, UtcTime, Utf8String,
};
use core::convert::TryInto;

#[cfg(feature = "oid")]
use crate::ObjectIdentifier;

/// DER encoder.
#[derive(Debug)]
pub struct Encoder<'a> {
    /// Buffer into which DER-encoded message is written
    bytes: Option<&'a mut [u8]>,

    /// Total number of bytes written to buffer so far
    position: Length,
}

impl<'a> Encoder<'a> {
    /// Create a new encoder with the given byte slice as a backing buffer.
    pub fn new(bytes: &'a mut [u8]) -> Self {
        Self {
            bytes: Some(bytes),
            position: Length::zero(),
        }
    }

    /// Encode a value which impls the [`Encodable`] trait.
    pub fn encode<T: Encodable>(&mut self, encodable: &T) -> Result<()> {
        if self.is_failed() {
            self.error(ErrorKind::Failed)?;
        }

        encodable.encode(self).map_err(|e| {
            self.bytes.take();
            e.nested(self.position)
        })
    }

    /// Return an error with the given [`ErrorKind`], annotating it with
    /// context about where the error occurred.
    pub fn error<T>(&mut self, kind: ErrorKind) -> Result<T> {
        self.bytes.take();
        Err(kind.at(self.position))
    }

    /// Did the decoding operation fail due to an error?
    pub fn is_failed(&self) -> bool {
        self.bytes.is_none()
    }

    /// Finish encoding to the buffer, returning a slice containing the data
    /// written to the buffer.
    pub fn finish(self) -> Result<&'a [u8]> {
        let position = self.position;

        match self.bytes {
            Some(bytes) => bytes
                .get(..self.position.into())
                .ok_or_else(|| ErrorKind::Truncated.at(position)),
            None => Err(ErrorKind::Failed.at(position)),
        }
    }

    /// Encode the provided value as an ASN.1 `BIT STRING`
    pub fn bit_string(&mut self, value: impl TryInto<BitString<'a>>) -> Result<()> {
        value
            .try_into()
            .or_else(|_| {
                self.error(ErrorKind::Value {
                    tag: Tag::BitString,
                })
            })
            .and_then(|value| self.encode(&value))
    }

    /// Encode the provided value as an ASN.1 `GeneralizedTime`
    pub fn generalized_time(&mut self, value: impl TryInto<GeneralizedTime>) -> Result<()> {
        value
            .try_into()
            .or_else(|_| {
                self.error(ErrorKind::Value {
                    tag: Tag::GeneralizedTime,
                })
            })
            .and_then(|value| self.encode(&value))
    }

    /// Encode the provided value as an ASN.1 `IA5String`
    pub fn ia5_string(&mut self, value: impl TryInto<Ia5String<'a>>) -> Result<()> {
        value
            .try_into()
            .or_else(|_| {
                self.error(ErrorKind::Value {
                    tag: Tag::Ia5String,
                })
            })
            .and_then(|value| self.encode(&value))
    }

    /// Encode an ASN.1 `NULL` value.
    pub fn null(&mut self) -> Result<()> {
        self.encode(&Null)
    }

    /// Encode the provided value as an ASN.1 `OCTET STRING`
    pub fn octet_string(&mut self, value: impl TryInto<OctetString<'a>>) -> Result<()> {
        value
            .try_into()
            .or_else(|_| {
                self.error(ErrorKind::Value {
                    tag: Tag::OctetString,
                })
            })
            .and_then(|value| self.encode(&value))
    }

    /// Encode an ASN.1 [`ObjectIdentifier`]
    #[cfg(feature = "oid")]
    #[cfg_attr(docsrs, doc(cfg(feature = "oid")))]
    pub fn oid(&mut self, value: impl TryInto<ObjectIdentifier>) -> Result<()> {
        value
            .try_into()
            .or_else(|_| {
                self.error(ErrorKind::Value {
                    tag: Tag::ObjectIdentifier,
                })
            })
            .and_then(|value| self.encode(&value))
    }

    /// Encode the provided value as an ASN.1 `PrintableString`
    pub fn printable_string(&mut self, value: impl TryInto<PrintableString<'a>>) -> Result<()> {
        value
            .try_into()
            .or_else(|_| {
                self.error(ErrorKind::Value {
                    tag: Tag::PrintableString,
                })
            })
            .and_then(|value| self.encode(&value))
    }

    /// Encode the provided value as an ASN.1 `UTCTime`
    pub fn utc_time(&mut self, value: impl TryInto<UtcTime>) -> Result<()> {
        value
            .try_into()
            .or_else(|_| self.error(ErrorKind::Value { tag: Tag::UtcTime }))
            .and_then(|value| self.encode(&value))
    }

    /// Encode the provided value as an ASN.1 `Utf8String`
    pub fn utf8_string(&mut self, value: impl TryInto<Utf8String<'a>>) -> Result<()> {
        value
            .try_into()
            .or_else(|_| {
                self.error(ErrorKind::Value {
                    tag: Tag::Utf8String,
                })
            })
            .and_then(|value| self.encode(&value))
    }

    /// Encode a sequence of values which impl the [`Encodable`] trait.
    // TODO(tarcieri): rename this to `message`, add `sequence` which handles nested encoder
    pub fn sequence(&mut self, encodables: &[&dyn Encodable]) -> Result<()> {
        let expected_len = sequence::encoded_len_inner(encodables)?;
        Header::new(Tag::Sequence, expected_len).and_then(|header| header.encode(self))?;

        let mut nested_encoder = Encoder::new(self.reserve(expected_len)?);

        for encodable in encodables {
            encodable.encode(&mut nested_encoder)?;
        }

        if nested_encoder.finish()?.len() == expected_len.into() {
            Ok(())
        } else {
            self.error(ErrorKind::Length { tag: Tag::Sequence })
        }
    }

    /// Reserve a portion of the internal buffer, updating the internal cursor
    /// position and returning a mutable slice.
    // TODO(tarcieri): make this private after implementing a nested `sequence` method
    pub fn reserve(&mut self, len: impl TryInto<Length>) -> Result<&mut [u8]> {
        let len = len
            .try_into()
            .or_else(|_| self.error(ErrorKind::Overflow))?;

        if len > self.remaining_len()? {
            self.error(ErrorKind::Overlength)?;
        }

        let end = (self.position + len).or_else(|e| self.error(e.kind()))?;
        let range = self.position.into()..end.into();
        let position = &mut self.position;

        // TODO(tarcieri): non-panicking version of this code
        // We ensure above that the buffer is untainted and there is sufficient
        // space to perform this slicing operation, however it would be nice to
        // have fully panic-free code.
        //
        // Unfortunately tainting the buffer on error is tricky to do when
        // potentially holding a reference to the buffer, and failure to taint
        // it would not uphold the invariant that any errors should taint it.
        let slice = &mut self.bytes.as_mut().expect("DER encoder tainted")[range];
        *position = end;

        Ok(slice)
    }

    /// Encode a single byte into the backing buffer.
    pub(crate) fn byte(&mut self, byte: u8) -> Result<()> {
        match self.reserve(1u8)?.first_mut() {
            Some(b) => {
                *b = byte;
                Ok(())
            }
            None => self.error(ErrorKind::Truncated),
        }
    }

    /// Encode the provided byte slice into the backing buffer.
    pub(crate) fn bytes(&mut self, slice: &[u8]) -> Result<()> {
        self.reserve(slice.len())?.copy_from_slice(slice);
        Ok(())
    }

    /// Get the size of the buffer in bytes.
    fn buffer_len(&self) -> Result<Length> {
        self.bytes
            .as_ref()
            .map(|bytes| bytes.len())
            .ok_or_else(|| ErrorKind::Failed.at(self.position))
            .and_then(TryInto::try_into)
    }

    /// Get the number of bytes still remaining in the buffer.
    fn remaining_len(&self) -> Result<Length> {
        self.buffer_len()?
            .to_usize()
            .checked_sub(self.position.into())
            .ok_or_else(|| ErrorKind::Truncated.at(self.position))
            .and_then(TryInto::try_into)
    }
}

#[cfg(test)]
mod tests {
    use super::Encoder;
    use crate::{Encodable, ErrorKind, Length};

    #[test]
    fn overlength_message() {
        let mut buffer = [];
        let mut encoder = Encoder::new(&mut buffer);
        let err = false.encode(&mut encoder).err().unwrap();
        assert_eq!(err.kind(), ErrorKind::Overlength);
        assert_eq!(err.position(), Some(Length::zero()));
    }
}
