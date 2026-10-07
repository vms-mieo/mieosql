//! Small file-writing helpers for frontend features (diagram / report export).

use base64::Engine as _;

use mieosql_core::{ErrorCode, MieoError};

use crate::dto::ApiResult;
use crate::service::Api;

impl Api {
    /// Write UTF-8 text to `path` (SVG / Markdown / HTML exports from the UI).
    pub fn write_text_file(&self, path: String, contents: String) -> ApiResult<()> {
        std::fs::write(&path, contents)
            .map_err(|e| MieoError::new(ErrorCode::Data, format!("cannot write {path}: {e}")))?;
        Ok(())
    }

    /// Write base64-decoded bytes to `path` (PNG export).
    pub fn write_binary_file(&self, path: String, base64_data: String) -> ApiResult<()> {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(base64_data.trim())
            .map_err(|e| MieoError::new(ErrorCode::Data, format!("invalid base64 payload: {e}")))?;
        std::fs::write(&path, bytes)
            .map_err(|e| MieoError::new(ErrorCode::Data, format!("cannot write {path}: {e}")))?;
        Ok(())
    }
}
