//! Swift-facing mirror of `pi_rpc::ExtensionUiRequest`/`ExtensionUiReply` —
//! pi's extension-UI dialog protocol (`select`/`confirm`/`input`/`editor`).
//! Plain UniFFI `Record`/`Enum` with `From` conversions, same pattern as
//! `row::RowRecord: From<pi_render::RowSpec>`.

/// Mirrors `pi_rpc::ExtensionUiRequest` field-for-field, minus `rest` — only
/// the four blocking dialog kinds (`select`/`confirm`/`input`/`editor`) are
/// ever converted into this type; see `apply()`'s scope-cut comment for why
/// `notify`/`setStatus`/`setWidget`/`setTitle`/`set_editor_text` (which
/// would otherwise only ever populate `rest`) aren't surfaced here.
#[derive(Clone, uniffi::Record)]
pub struct ExtensionDialogRecord {
    pub id: String,
    pub method: String,
    pub title: Option<String>,
    pub message: Option<String>,
    pub options: Option<Vec<String>>,
    pub placeholder: Option<String>,
    pub prefill: Option<String>,
    pub timeout: Option<u64>,
}

impl From<pi_rpc::ExtensionUiRequest> for ExtensionDialogRecord {
    fn from(req: pi_rpc::ExtensionUiRequest) -> Self {
        Self {
            id: req.id,
            method: req.method,
            title: req.title,
            message: req.message,
            options: req.options,
            placeholder: req.placeholder,
            prefill: req.prefill,
            timeout: req.timeout,
        }
    }
}

/// Mirrors `pi_rpc::ExtensionUiReply` — the answer to an
/// `ExtensionDialogRecord`, sent back via `PiSession::
/// reply_extension_dialog`.
#[derive(Clone, uniffi::Enum)]
pub enum ExtensionDialogReply {
    Value { value: String },
    Confirmed { confirmed: bool },
    Cancelled,
}

impl From<ExtensionDialogReply> for pi_rpc::ExtensionUiReply {
    fn from(reply: ExtensionDialogReply) -> Self {
        match reply {
            ExtensionDialogReply::Value { value } => Self::Value(value),
            ExtensionDialogReply::Confirmed { confirmed } => Self::Confirmed(confirmed),
            ExtensionDialogReply::Cancelled => Self::Cancelled,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dialog_record_conversion_preserves_every_field() {
        let mut rest = serde_json::Map::new();
        rest.insert("extra".to_string(), serde_json::Value::Bool(true));
        let req = pi_rpc::ExtensionUiRequest {
            id: "req-1".to_string(),
            method: "select".to_string(),
            title: Some("Pick one".to_string()),
            message: Some("Choose an option".to_string()),
            options: Some(vec!["a".to_string(), "b".to_string()]),
            placeholder: None,
            prefill: None,
            timeout: Some(5000),
            rest,
        };
        let record = ExtensionDialogRecord::from(req);
        assert_eq!(record.id, "req-1");
        assert_eq!(record.method, "select");
        assert_eq!(record.title.as_deref(), Some("Pick one"));
        assert_eq!(record.message.as_deref(), Some("Choose an option"));
        assert_eq!(record.options, Some(vec!["a".to_string(), "b".to_string()]));
        assert_eq!(record.timeout, Some(5000));
    }

    #[test]
    fn reply_conversions_map_to_the_matching_pi_rpc_variant() {
        assert!(matches!(
            pi_rpc::ExtensionUiReply::from(ExtensionDialogReply::Value {
                value: "hi".to_string()
            }),
            pi_rpc::ExtensionUiReply::Value(v) if v == "hi"
        ));
        assert!(matches!(
            pi_rpc::ExtensionUiReply::from(ExtensionDialogReply::Confirmed { confirmed: true }),
            pi_rpc::ExtensionUiReply::Confirmed(true)
        ));
        assert!(matches!(
            pi_rpc::ExtensionUiReply::from(ExtensionDialogReply::Cancelled),
            pi_rpc::ExtensionUiReply::Cancelled
        ));
    }
}
