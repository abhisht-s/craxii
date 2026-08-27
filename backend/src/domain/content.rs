//! Versioned text content and its storage-neutral canonical hash grammar.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

use super::{DomainValidationError, DomainValidationKind, Sha256Digest};

const CONTENT_MAGIC: &[u8] = b"craxii.content";
const TEXT_BLOCK_TAG: u8 = 0x01;

/// The maximum combined UTF-8 text payload in one V0 message.
pub const MAX_CONTENT_TEXT_BYTES: usize = 65_536;

/// The only content codec version supported by V0.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ContentVersion;

impl ContentVersion {
    /// V0's exact numeric content version.
    pub const V1: Self = Self;

    /// Returns the canonical unsigned-byte value included in content hashes.
    #[must_use]
    pub const fn get(self) -> u8 {
        1
    }
}

impl Serialize for ContentVersion {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_u8(self.get())
    }
}

impl<'de> Deserialize<'de> for ContentVersion {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = u8::deserialize(deserializer)?;
        if value == Self::V1.get() {
            Ok(Self::V1)
        } else {
            Err(de::Error::custom("content version must be 1"))
        }
    }
}

/// One ordered V0 content block.
#[derive(Clone, Eq, PartialEq)]
pub enum ContentBlock {
    /// Exact UTF-8 text with identity normalization.
    Text(String),
}

impl ContentBlock {
    /// Constructs a nonempty text block without trimming or normalization.
    pub fn text(value: impl Into<String>) -> Result<Self, DomainValidationError> {
        let value = value.into();
        if value.is_empty() {
            return Err(DomainValidationError::new(
                DomainValidationKind::InvalidText,
            ));
        }
        Ok(Self::Text(value))
    }

    /// Returns the exact preserved text.
    #[must_use]
    pub fn as_text(&self) -> &str {
        match self {
            Self::Text(value) => value,
        }
    }

    fn utf8_len(&self) -> usize {
        self.as_text().len()
    }
}

impl fmt::Debug for ContentBlock {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ContentBlock::Text")
            .field("utf8_bytes", &self.utf8_len())
            .field("text", &"[REDACTED]")
            .finish()
    }
}

/// Validated ordered content blocks for one immutable message.
#[derive(Clone, Eq, PartialEq)]
pub struct MessageContent {
    blocks: Vec<ContentBlock>,
}

impl MessageContent {
    /// Validates V1's nonempty-block and combined 64 KiB contracts.
    pub fn try_new(blocks: Vec<ContentBlock>) -> Result<Self, DomainValidationError> {
        if blocks.is_empty() {
            return Err(DomainValidationError::new(
                DomainValidationKind::InvalidContent,
            ));
        }

        let _block_count = u32::try_from(blocks.len())
            .map_err(|_| DomainValidationError::new(DomainValidationKind::InvalidContent))?;
        let mut total = 0_usize;
        for block in &blocks {
            let length = block.utf8_len();
            if length == 0 || u64::try_from(length).is_err() {
                return Err(DomainValidationError::new(
                    DomainValidationKind::InvalidText,
                ));
            }
            total = total
                .checked_add(length)
                .ok_or_else(|| DomainValidationError::new(DomainValidationKind::InvalidContent))?;
            if total > MAX_CONTENT_TEXT_BYTES {
                return Err(DomainValidationError::new(
                    DomainValidationKind::InvalidContent,
                ));
            }
        }

        Ok(Self { blocks })
    }

    /// Returns the exact content codec version.
    #[must_use]
    pub const fn version(&self) -> ContentVersion {
        ContentVersion::V1
    }

    /// Returns the immutable ordered block view.
    #[must_use]
    pub fn blocks(&self) -> &[ContentBlock] {
        &self.blocks
    }

    /// Returns the combined UTF-8 payload length.
    #[must_use]
    pub fn total_text_bytes(&self) -> usize {
        self.blocks.iter().map(ContentBlock::utf8_len).sum()
    }

    /// Encodes the exact V1 binary grammar used as SHA-256 input.
    #[must_use]
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let block_count =
            u32::try_from(self.blocks.len()).expect("validated content block count must fit u32");
        let framing_bytes = self
            .blocks
            .len()
            .checked_mul(9)
            .and_then(|value| value.checked_add(CONTENT_MAGIC.len() + 1 + 4))
            .expect("validated content framing size must fit usize");
        let capacity = framing_bytes
            .checked_add(self.total_text_bytes())
            .expect("validated canonical content size must fit usize");

        let mut canonical = Vec::with_capacity(capacity);
        canonical.extend_from_slice(CONTENT_MAGIC);
        canonical.push(self.version().get());
        canonical.extend_from_slice(&block_count.to_be_bytes());
        for block in &self.blocks {
            canonical.push(TEXT_BLOCK_TAG);
            let text = block.as_text().as_bytes();
            let length = u64::try_from(text.len())
                .expect("validated content block byte length must fit u64");
            canonical.extend_from_slice(&length.to_be_bytes());
            canonical.extend_from_slice(text);
        }
        canonical
    }

    /// Hashes only the canonical ordered content bytes.
    #[must_use]
    pub fn content_sha256(&self) -> Sha256Digest {
        Sha256Digest::hash_bytes(&self.canonical_bytes())
    }
}

impl fmt::Debug for MessageContent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MessageContent")
            .field("version", &self.version())
            .field("block_count", &self.blocks.len())
            .field("total_text_bytes", &self.total_text_bytes())
            .field("content_sha256", &self.content_sha256())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn content(texts: &[&str]) -> MessageContent {
        MessageContent::try_new(
            texts
                .iter()
                .map(|text| ContentBlock::text(*text).unwrap())
                .collect(),
        )
        .unwrap()
    }

    #[test]
    fn exact_canonical_bytes_and_digest_are_golden() {
        let content = content(&["hi", "é"]);
        let expected = [
            b"craxii.content".as_slice(),
            &[0x01],
            &[0x00, 0x00, 0x00, 0x02],
            &[0x01],
            &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02],
            b"hi",
            &[0x01],
            &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02],
            "é".as_bytes(),
        ]
        .concat();

        assert_eq!(content.canonical_bytes(), expected);
        assert_eq!(
            content.content_sha256().to_string(),
            "aa277aa62fe202173a61976d5618655227471bb659d0db48b5638f6b167a72a4"
        );
    }

    #[test]
    fn framing_fields_and_multibyte_lengths_are_exact_big_endian() {
        assert_eq!(serde_json::to_string(&ContentVersion::V1).unwrap(), "1");
        assert!(serde_json::from_str::<ContentVersion>("2").is_err());
        let bytes = content(&["é"]).canonical_bytes();
        let version_offset = CONTENT_MAGIC.len();
        assert_eq!(bytes[version_offset], 0x01);
        assert_eq!(
            &bytes[version_offset + 1..version_offset + 5],
            &1_u32.to_be_bytes()
        );
        assert_eq!(bytes[version_offset + 5], TEXT_BLOCK_TAG);
        assert_eq!(
            &bytes[version_offset + 6..version_offset + 14],
            &2_u64.to_be_bytes()
        );
        assert_eq!(&bytes[version_offset + 14..], "é".as_bytes());

        let mut other_version = bytes;
        other_version[version_offset] = 2;
        assert_ne!(
            Sha256Digest::hash_bytes(&other_version),
            content(&["é"]).content_sha256()
        );
    }

    #[test]
    fn order_and_block_boundaries_are_collision_separators() {
        let first = content(&["a", "bc"]);
        let second = content(&["ab", "c"]);
        let reversed = content(&["bc", "a"]);
        assert_ne!(first.canonical_bytes(), second.canonical_bytes());
        assert_ne!(first.content_sha256(), second.content_sha256());
        assert_ne!(first.canonical_bytes(), reversed.canonical_bytes());
        assert_ne!(first.content_sha256(), reversed.content_sha256());
    }

    #[test]
    fn text_normalization_is_identity_and_whitespace_is_preserved() {
        let composed = content(&["é"]);
        let decomposed = content(&["e\u{301}"]);
        assert_ne!(composed.canonical_bytes(), decomposed.canonical_bytes());
        assert_ne!(composed.content_sha256(), decomposed.content_sha256());

        let spaced = content(&["  \n\t  "]);
        assert_eq!(spaced.blocks()[0].as_text(), "  \n\t  ");
        assert!(spaced.canonical_bytes().ends_with(b"  \n\t  "));
    }

    #[test]
    fn empty_blocks_and_zero_blocks_are_rejected() {
        assert_eq!(
            ContentBlock::text("").unwrap_err().kind(),
            DomainValidationKind::InvalidText
        );
        assert_eq!(
            MessageContent::try_new(Vec::new()).unwrap_err().kind(),
            DomainValidationKind::InvalidContent
        );
        assert_eq!(
            MessageContent::try_new(vec![ContentBlock::Text(String::new())])
                .unwrap_err()
                .kind(),
            DomainValidationKind::InvalidText
        );
    }

    #[test]
    fn combined_limit_accepts_exactly_65536_and_rejects_65537() {
        let exact = MessageContent::try_new(vec![
            ContentBlock::text("a".repeat(32_768)).unwrap(),
            ContentBlock::text("b".repeat(32_768)).unwrap(),
        ])
        .unwrap();
        assert_eq!(exact.total_text_bytes(), 65_536);

        let over = MessageContent::try_new(vec![
            ContentBlock::text("a".repeat(65_536)).unwrap(),
            ContentBlock::text("b").unwrap(),
        ])
        .unwrap_err();
        assert_eq!(over.kind(), DomainValidationKind::InvalidContent);
    }

    #[test]
    fn json_shapes_are_not_hash_input() {
        let content = content(&["same"]);
        let object_first = br#"{"type":"text","text":"same"}"#;
        let object_second = br#"{"text":"same","type":"text"}"#;
        assert_ne!(object_first, object_second);
        assert_eq!(
            content.content_sha256(),
            Sha256Digest::hash_bytes(&content.canonical_bytes())
        );
    }
}
