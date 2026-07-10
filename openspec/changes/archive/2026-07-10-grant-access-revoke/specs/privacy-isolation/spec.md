## ADDED Requirements

### Requirement: A user can revoke and list the grants they made

The data owner SHALL be able to revoke a cross-user grant they previously made and to list their outstanding grants. A `revoke_access` tool SHALL remove grants matching the given grantee, narrowed by an optional scope (exact match); when the scope is omitted, every grant the owner made to that grantee SHALL be removed. Revocation SHALL take effect immediately, so the data-layer guard denies the revoked access on its next decision. A `list_access` tool SHALL return the grants the acting user currently holds as owner (grantee and scope). Guest SHALL NOT revoke any grant and SHALL receive an empty list. Revoking a grant that does not exist SHALL succeed and report zero grants removed, revealing nothing about other users. The grant store SHALL be updated under the same lock as grant creation so concurrent grant and revoke operations MUST NOT clobber each other.

#### Scenario: revoking a grant removes access

- **WHEN** owner A revokes the grant that let B access A's scope, and B then attempts that access
- **THEN** the revocation removes the grant and B's access is denied

#### Scenario: revoking without a scope removes all grants to that grantee

- **WHEN** owner A revokes B with no scope while A holds multiple scoped grants to B
- **THEN** every grant A made to B is removed and B retains no access to A's data

#### Scenario: listing shows the owner's outstanding grants

- **WHEN** the acting user lists their access grants
- **THEN** each grant they made is returned with its grantee and scope, and no other owner's grants appear

#### Scenario: guest cannot revoke or enumerate grants

- **WHEN** the acting principal is Guest and attempts to revoke or list grants
- **THEN** the revoke is refused and the list is empty

#### Scenario: revoking a non-existent grant reveals nothing

- **WHEN** owner A revokes a grantee or scope that A never granted
- **THEN** the operation succeeds reporting zero grants removed and discloses nothing about whether that grantee exists