//! Domain refcount: `<Domain>.enable` on first use, `<Domain>.disable` after last use.
//!
//! Each user holds a domain under a name of its own ("net", "record:rec-1",
//! "adblock.measure:3"). The holder names make the count idempotent (holding twice counts once)
//! and easy to inspect ([`crate::CdpShare::holders`]).
//!
//! **Parameters.** Holders may ask for different `enable` parameters (net wants
//! `Network.enable {maxPostDataSize: 65536}`, record passes none). They are merged: for a field
//! that several holders set, numbers take the largest value, booleans are OR-ed, anything else
//! keeps the value of the first holder (by name). When the merged parameters change, `enable`
//! is sent again (Chromium accepts a repeated `enable` with new parameters). When a holder
//! leaves, the merged parameters are *not* shrunk again: a larger buffer does no harm.

use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;

use bmcp_core::BackendResult;
use serde_json::{Map, Value, json};

use crate::{TabShare, lock};

/// Domains that are enabled through the refcount but never disabled by it. `Page` is also
/// enabled by code outside this crate (identity's stealth setup), it is invisible to the page,
/// and nobody ever disabled it before: switching it off when our last holder leaves could
/// break those users.
pub const NEVER_DISABLE: &[&str] = &["Page"];

/// One tab's domain state.
#[derive(Default)]
pub(crate) struct TabDomains {
    /// Held while sending enable/disable, so two changes never interleave their CDP calls.
    config: tokio::sync::Mutex<()>,
    data: Mutex<Data>,
}

#[derive(Default)]
struct Data {
    /// domain → holder → the parameters it asked for.
    held: HashMap<String, BTreeMap<String, Value>>,
    /// domain → parameters of the `enable` we sent last. Present = enabled by us.
    sent: HashMap<String, Value>,
}

impl TabDomains {
    pub fn holders(&self, domain: &str) -> Vec<String> {
        lock(&self.data)
            .held
            .get(domain)
            .map(|h| h.keys().cloned().collect())
            .unwrap_or_default()
    }
}

pub(crate) async fn acquire(
    ts: &TabShare,
    domain: &str,
    holder: &str,
    params: Value,
) -> BackendResult<()> {
    let _config = ts.domains.config.lock().await;
    let (previous, wanted) = {
        let mut data = lock(&ts.domains.data);
        let holders = data.held.entry(domain.to_string()).or_default();
        let previous = holders.insert(holder.to_string(), params);
        let wanted = merged(holders);
        if data.sent.get(domain) == Some(&wanted) {
            return Ok(()); // already enabled with these parameters
        }
        (previous, wanted)
    };
    match ts.cdp(&format!("{domain}.enable"), wanted.clone()).await {
        Ok(_) => {
            lock(&ts.domains.data)
                .sent
                .insert(domain.to_string(), wanted);
            Ok(())
        }
        Err(err) => {
            // Undo: this holder does not hold the domain (or keeps its old parameters).
            let mut data = lock(&ts.domains.data);
            if let Some(holders) = data.held.get_mut(domain) {
                match previous {
                    Some(old) => holders.insert(holder.to_string(), old),
                    None => holders.remove(holder),
                };
                if holders.is_empty() {
                    data.held.remove(domain);
                }
            }
            Err(err)
        }
    }
}

pub(crate) async fn release(ts: &TabShare, domain: &str, holder: &str) -> BackendResult<()> {
    let _config = ts.domains.config.lock().await;
    {
        let mut data = lock(&ts.domains.data);
        let Some(holders) = data.held.get_mut(domain) else {
            return Ok(());
        };
        if holders.remove(holder).is_none() || !holders.is_empty() {
            return Ok(()); // not a holder, or somebody else still needs the domain
        }
        data.held.remove(domain);
        if NEVER_DISABLE.contains(&domain) || data.sent.remove(domain).is_none() {
            return Ok(());
        }
    }
    ts.cdp(&format!("{domain}.disable"), json!({}))
        .await
        .map(|_| ())
}

/// All holders' parameters merged into one object (see the module docs).
fn merged(holders: &BTreeMap<String, Value>) -> Value {
    let mut out = Map::new();
    for params in holders.values() {
        let Some(fields) = params.as_object() else {
            continue;
        };
        for (key, value) in fields {
            match out.get_mut(key) {
                None => {
                    out.insert(key.clone(), value.clone());
                }
                Some(have) => {
                    if let (Some(a), Some(b)) = (have.as_f64(), value.as_f64()) {
                        if b > a {
                            *have = value.clone();
                        }
                    } else if let (Some(a), Some(b)) = (have.as_bool(), value.as_bool()) {
                        *have = json!(a || b);
                    }
                }
            }
        }
    }
    Value::Object(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parameters_merge_to_the_largest() {
        let holders = BTreeMap::from([
            (
                "a".to_string(),
                json!({ "maxPostDataSize": 65536, "x": "first" }),
            ),
            ("b".to_string(), json!({})),
            (
                "c".to_string(),
                json!({ "maxPostDataSize": 1024, "maxTotalBufferSize": 10, "x": "second", "f": true }),
            ),
        ]);
        assert_eq!(
            merged(&holders),
            json!({ "maxPostDataSize": 65536, "maxTotalBufferSize": 10, "x": "first", "f": true })
        );
        assert_eq!(merged(&BTreeMap::new()), json!({}));
    }
}
