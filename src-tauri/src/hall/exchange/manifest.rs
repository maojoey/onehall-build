use super::types::*;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
pub fn validate_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > 240
        || name.trim() != name
        || name == "."
        || name == ".."
        || name.ends_with('.')
        || name
            .chars()
            .any(|c| c.is_control() || "/\\:<>\"|?*".contains(c))
    {
        return Err(ExchangeError::Invalid);
    }
    let stem = name
        .split('.')
        .next()
        .unwrap_or("")
        .trim_end()
        .to_uppercase();
    if matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CLOCK$" | "CONIN$" | "CONOUT$"
    ) || stem
        .strip_prefix("COM")
        .or_else(|| stem.strip_prefix("LPT"))
        .is_some_and(|suffix| {
            matches!(
                suffix,
                "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
            )
        })
    {
        return Err(ExchangeError::Invalid);
    }
    Ok(())
}
pub(super) fn name_key(name: &str) -> Result<String> {
    validate_name(name)?;
    Ok(name.to_uppercase())
}
pub(super) fn validate_hash(hash: &str) -> Result<()> {
    identity(hash)?;
    if hash.len() != 64
        || !hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(ExchangeError::Invalid);
    }
    Ok(())
}
pub fn build(
    manifest_id: &str,
    mut inputs: Vec<ObjectRef>,
    mut outputs: Vec<OutputSlot>,
) -> Result<ExchangeManifest> {
    uuid(manifest_id)?;
    if inputs.len() + outputs.len() > MAX_MANIFEST_ITEMS {
        return Err(ExchangeError::Quota);
    }
    let mut ids = HashSet::new();
    let mut names = HashSet::new();
    let mut total = 0u64;
    for object in &inputs {
        uuid(&object.object_id)?;
        validate_hash(&object.sha256)?;
        if !ids.insert(object.object_id.as_str()) || !names.insert(name_key(&object.display_name)?)
        {
            return Err(ExchangeError::Invalid);
        }
        if object.size > MAX_FILE_BYTES {
            return Err(ExchangeError::Quota);
        }
        total = total.checked_add(object.size).ok_or(ExchangeError::Quota)?;
        if total > MAX_TOTAL_BYTES {
            return Err(ExchangeError::Quota);
        }
    }
    for slot in &outputs {
        uuid(&slot.slot_id)?;
        if !ids.insert(slot.slot_id.as_str()) || !names.insert(name_key(&slot.display_name)?) {
            return Err(ExchangeError::Invalid);
        }
    }
    inputs.sort_by(|a, b| a.object_id.cmp(&b.object_id));
    outputs.sort_by(|a, b| a.slot_id.cmp(&b.slot_id));
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Canonical<'a> {
        schema_version: u32,
        manifest_id: &'a str,
        inputs: &'a [ObjectRef],
        outputs: &'a [OutputSlot],
    }
    let bytes = serde_json::to_vec(&Canonical {
        schema_version: 1,
        manifest_id,
        inputs: &inputs,
        outputs: &outputs,
    })
    .map_err(|_| ExchangeError::Invalid)?;
    let manifest = ExchangeManifest {
        schema_version: 1,
        manifest_id: manifest_id.into(),
        inputs,
        outputs,
        digest: format!("{:x}", Sha256::digest(bytes)),
    };
    if serde_json::to_vec(&manifest)
        .map_err(|_| ExchangeError::Invalid)?
        .len()
        > MAX_MANIFEST_BYTES
    {
        return Err(ExchangeError::Quota);
    }
    Ok(manifest)
}
pub fn verify(manifest: &ExchangeManifest) -> Result<()> {
    if manifest.schema_version != 1
        || build(
            &manifest.manifest_id,
            manifest.inputs.clone(),
            manifest.outputs.clone(),
        )?
        .digest
            != manifest.digest
    {
        return Err(ExchangeError::Invalid);
    }
    Ok(())
}
