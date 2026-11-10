// SPDX-License-Identifier: Apache-2.0
//! Policy Adapters ([ADR 0005]): the policy sees the resource, and knows no app's vocabulary.
//!
//! [ADR 0005]: https://docs.addaui.com/nuah/adr/0005-policy-sees-the-resource-results-carry-what-you-may-do/

use nuah_kernel::*;
use serde_json::{json, Value};
use std::sync::Arc;

struct AllowAll;
impl Policy for AllowAll {
    fn name(&self) -> &str {
        "@nuah/policy-allow-all"
    }
    fn allows(&self, _who: &Identity, permission: &str, grant: Option<&Grant>, _t: &PermissionTable) -> bool {
        // Roles do not matter here; what the resource grants does ([ADR 0018]). Without a resource, everything.
        //
        // [ADR 0018]: https://docs.addaui.com/nuah/adr/0018-allow-all-ignores-roles-but-still-honours-what-the-resource-grants/
        grant.map_or(true, |g| g.permissions.iter().any(|p| p == permission))
    }
}

struct Roles;
impl Policy for Roles {
    fn name(&self) -> &str {
        "@nuah/policy-roles"
    }
    fn allows(&self, who: &Identity, permission: &str, grant: Option<&Grant>, table: &PermissionTable) -> bool {
        let by_role = table.get(permission).is_some_and(|roles| roles.iter().any(|r| who.has_role(r)));
        let by_resource = grant.map_or(true, |g| g.permissions.iter().any(|p| p == permission));
        by_role && by_resource
    }
}

pub struct PolicyAllowAll;
impl Plugin for PolicyAllowAll {
    fn manifest(&self) -> Manifest {
        serde_json::from_value(json!({"id":"@nuah/policy-allow-all","version":"0.2.0","name":"Allow everything","description":"Single-user setups: every Subject may do whatever the resource lets them","fills":"policy"})).unwrap()
    }
    fn fill(&self, _s: &Value) -> Result<Option<PortImpl>> {
        Ok(Some(PortImpl::Policy(Arc::new(AllowAll))))
    }
}

pub struct PolicyRoles;
impl Plugin for PolicyRoles {
    fn manifest(&self) -> Manifest {
        serde_json::from_value(json!({"id":"@nuah/policy-roles","version":"0.2.0","name":"Roles","description":"A Permission is held by the Roles the manifests and the Profile name, and also needs what the resource grants","fills":"policy"})).unwrap()
    }
    fn fill(&self, _s: &Value) -> Result<Option<PortImpl>> {
        Ok(Some(PortImpl::Policy(Arc::new(Roles))))
    }
}
