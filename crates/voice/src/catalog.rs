//! Whisper models that can be downloaded for Local Whisper: the ggml files of the official
//! `ggerganov/whisper.cpp` repository on Hugging Face, with the size and SHA-256 it publishes
//! (Git LFS object ids from `api/models/ggerganov/whisper.cpp/tree/main`).

use schemars::JsonSchema;
use serde::Serialize;

/// Where the model files come from.
pub const HF_BASE: &str = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main";

/// One downloadable model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
pub struct ModelInfo {
    /// Id used by the commands (`base`, `large-v3-turbo-q5_0`).
    pub id: &'static str,
    /// Display name.
    pub name: &'static str,
    /// File name in the repository and in `data/voice/models`.
    pub file: &'static str,
    /// Exact size in bytes.
    pub size: u64,
    /// SHA-256 (hex) of the file.
    pub sha256: &'static str,
    /// Recommended for Ukrainian and other non-English languages.
    pub recommended: bool,
    /// Rough quality: 1 (rough) … 5 (best).
    pub quality: u8,
    /// Rough speed on a CPU: 1 (slow) … 5 (fast).
    pub speed: u8,
}

/// Every model offered, smallest first. Preference when none is chosen: [`PREFERRED`].
pub const MODELS: &[ModelInfo] = &[
    ModelInfo {
        id: "tiny",
        name: "Tiny",
        file: "ggml-tiny.bin",
        size: 77_691_713,
        sha256: "be07e048e1e599ad46341c8d2a135645097a538221678b7acdd1b1919c6e1b21",
        recommended: false,
        quality: 1,
        speed: 5,
    },
    ModelInfo {
        id: "base",
        name: "Base",
        file: "ggml-base.bin",
        size: 147_951_465,
        sha256: "60ed5bc3dd14eea856493d334349b405782ddcaf0028d4b5df4088345fba2efe",
        recommended: false,
        quality: 2,
        speed: 5,
    },
    ModelInfo {
        id: "small",
        name: "Small",
        file: "ggml-small.bin",
        size: 487_601_967,
        sha256: "1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b",
        recommended: false,
        quality: 3,
        speed: 3,
    },
    ModelInfo {
        id: "large-v3-turbo-q5_0",
        name: "Large v3 Turbo (q5_0)",
        file: "ggml-large-v3-turbo-q5_0.bin",
        size: 574_041_195,
        sha256: "394221709cd5ad1f40c46e6031ca61bce88931e6e088c188294c6d5a55ffa7e2",
        recommended: true,
        quality: 4,
        speed: 3,
    },
    ModelInfo {
        id: "medium-q5_0",
        name: "Medium (q5_0)",
        file: "ggml-medium-q5_0.bin",
        size: 539_212_467,
        sha256: "19fea4b380c3a618ec4723c3eef2eb785ffba0d0538cf43f8f235e7b3b34220f",
        recommended: false,
        quality: 4,
        speed: 2,
    },
    ModelInfo {
        id: "large-v3-turbo",
        name: "Large v3 Turbo",
        file: "ggml-large-v3-turbo.bin",
        size: 1_624_555_275,
        sha256: "1fc70f774d38eb169993ac391eea357ef47c88757ef72ee5943879b7e8e2bc69",
        recommended: false,
        quality: 5,
        speed: 2,
    },
];

/// Which installed model to use when the settings name none: best quality first.
pub const PREFERRED: &[&str] = &[
    "large-v3-turbo-q5_0",
    "large-v3-turbo",
    "medium-q5_0",
    "small",
    "base",
    "tiny",
];

pub fn model(id: &str) -> Option<&'static ModelInfo> {
    MODELS.iter().find(|m| m.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_is_consistent() {
        for m in MODELS {
            assert_eq!(m.sha256.len(), 64, "{}", m.id);
            assert!(m.sha256.chars().all(|c| c.is_ascii_hexdigit()));
            assert_eq!(m.file, format!("ggml-{}.bin", m.id));
            assert!(PREFERRED.contains(&m.id), "{} not in PREFERRED", m.id);
        }
        assert_eq!(PREFERRED.len(), MODELS.len());
        assert_eq!(MODELS.iter().filter(|m| m.recommended).count(), 1);
        assert!(model("base").is_some() && model("huge").is_none());
    }
}
