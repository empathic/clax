//! The JSON view of a grant.

use clax_core::rights::Rights;
use clax_core::sig;
use clax_store::people::Grant;
use serde_json::{Value, json};

/// A grant's view for the owner: never a hash or a code.
pub fn grant_view(g: &Grant) -> Value {
    let fingerprint = sig::decode_public(&g.client_key)
        .map(|k| sig::fingerprint(&k))
        .ok();
    json!({
        "id": g.id,
        "status": g.status,
        "via": g.via,
        "client": {"name": g.client_name, "key": fingerprint.filter(|_| !g.bearer), "peer_ip": g.peer_ip},
        "bearer": g.bearer,
        "person": g.person.as_ref().map(|p| json!({"public_id": p, "display_name": g.person_name})),
        "owner": g.owner,
        "narrow": g.narrow.as_ref().map(Rights::to_json),
        "wrong_codes": g.wrong_codes,
        "created_at": g.created_at,
        "decided_at": g.decided_at,
        "last_used_at": g.last_used_at,
        "ended_at": g.ended_at,
        "ended_by": g.ended_by,
    })
}
