#!/usr/bin/env bash
# Builds the e2e realm in a running Keycloak with kcadm, mirroring the
# production setup that soju and Backroom rely on:
#
#  - soju: confidential client soju uses for token introspection, with an
#    `access` client role.
#  - soju_roles client scope: soju.access in its Scope tab, a client-role
#    mapper (claim `soju_roles`) and Audience Resolve, so only users with
#    soju.access get `aud: soju` (Keycloak >= 26.6.2 requires that for
#    introspection).
#  - backroom: public client, PKCE S256, full scope off, soju_roles default.
#  - gated-browser flow: sign in, then a conditional "gate soju" subflow that
#    denies users without soju.access when the client has soju_roles.
#  - e2e-direct: password-grant client that skips the browser flow, so tests
#    can get a token for a user the gate would have refused.
#  - users: alice (in group soju-users, has soju.access), mallory (no role).
#    Password = username.
#
# Used by scripts/e2e.sh realm to regenerate realm.json; not run in CI.
#   build-realm.sh <keycloak container>
set -euo pipefail

C=$1
R=e2e
kc() { docker exec "$C" /opt/keycloak/bin/kcadm.sh "$@"; }
id_of() { kc "$@" --fields id --format csv --noquotes | head -1; }
# Execution id in flow $1 whose displayName is $2.
exec_id() {
	kc get "authentication/flows/$1/executions" -r $R | python3 -I -c '
import json, sys
name = sys.argv[1]
print(next(e["id"] for e in json.load(sys.stdin) if e["displayName"] == name))' "$2"
}
require() { kc update "authentication/flows/$1/executions" -r $R -b "{\"id\":\"$2\",\"requirement\":\"$3\"}"; }

kc config credentials --server http://localhost:8080 --realm master --user admin --password admin

kc create realms -s realm=$R -s enabled=true -s sslRequired=none -s accessTokenLifespan=300

# soju and its role
kc create clients -r $R -s clientId=soju -s publicClient=false -s secret=soju-e2e-secret \
	-s standardFlowEnabled=false -s directAccessGrantsEnabled=false -s serviceAccountsEnabled=false
SOJU=$(id_of get clients -r $R -q clientId=soju)
kc create "clients/$SOJU/roles" -r $R -s name=access
ACCESS=$(id_of get "clients/$SOJU/roles/access" -r $R)

kc create groups -r $R -s name=soju-users
GROUP=$(id_of get groups -r $R -q search=soju-users)
kc add-roles -r $R --gname soju-users --cclientid soju --rolename access

for u in alice mallory; do
	kc create users -r $R -s username=$u -s enabled=true -s email=$u@e2e.test -s emailVerified=true \
		-s firstName=$u -s lastName=E2E
	kc set-password -r $R --username $u --new-password $u
done
ALICE=$(id_of get users -r $R -q username=alice -q exact=true)
kc update "users/$ALICE/groups/$GROUP" -r $R -n

# soju_roles client scope
kc create client-scopes -r $R -s name=soju_roles -s protocol=openid-connect \
	-s 'attributes."include.in.token.scope"=true'
SCOPE=$(kc get client-scopes -r $R | python3 -I -c '
import json, sys
print(next(s["id"] for s in json.load(sys.stdin) if s["name"] == "soju_roles"))')
kc create "client-scopes/$SCOPE/protocol-mappers/models" -r $R -s 'name=soju roles' -s protocol=openid-connect \
	-s protocolMapper=oidc-usermodel-client-role-mapper \
	-s 'config."usermodel.clientRoleMapping.clientId"=soju' -s 'config."claim.name"=soju_roles' \
	-s config.multivalued=true -s 'config."jsonType.label"=String' \
	-s 'config."access.token.claim"=true' -s 'config."introspection.token.claim"=true'
kc create "client-scopes/$SCOPE/protocol-mappers/models" -r $R -s 'name=soju audience resolve' \
	-s protocol=openid-connect -s protocolMapper=oidc-audience-resolve-mapper \
	-s 'config."access.token.claim"=true' -s 'config."introspection.token.claim"=true'
kc create "client-scopes/$SCOPE/scope-mappings/clients/$SOJU" -r $R -b "[{\"id\":\"$ACCESS\",\"name\":\"access\"}]"

# Clients that get tokens for soju
kc create clients -r $R -s clientId=backroom -s publicClient=true -s standardFlowEnabled=true \
	-s directAccessGrantsEnabled=false -s fullScopeAllowed=false \
	-s 'redirectUris=["http://127.0.0.1:47615/callback"]' -s 'attributes."pkce.code.challenge.method"=S256'
kc create clients -r $R -s clientId=e2e-direct -s publicClient=true -s standardFlowEnabled=false \
	-s directAccessGrantsEnabled=true -s fullScopeAllowed=false
for client in backroom e2e-direct; do
	ID=$(id_of get clients -r $R -q clientId=$client)
	kc update "clients/$ID/default-client-scopes/$SCOPE" -r $R -n
done

# gated-browser: [sign-in: cookie | forms] then [gate-soju: scope & !role -> deny]
kc create authentication/flows -r $R -s alias=gated-browser -s providerId=basic-flow -s topLevel=true -s builtIn=false
kc create authentication/flows/gated-browser/executions/flow -r $R -s alias=sign-in -s type=basic-flow
kc create authentication/flows/sign-in/executions/execution -r $R -s provider=auth-cookie
kc create authentication/flows/sign-in/executions/flow -r $R -s alias=sign-in-forms -s type=basic-flow
kc create authentication/flows/sign-in-forms/executions/execution -r $R -s provider=auth-username-password-form
kc create authentication/flows/gated-browser/executions/flow -r $R -s alias=gate-soju -s type=basic-flow
for p in conditional-client-scope conditional-user-role deny-access-authenticator; do
	kc create authentication/flows/gate-soju/executions/execution -r $R -s provider=$p
done

require gated-browser "$(exec_id gated-browser sign-in)" REQUIRED
require sign-in "$(exec_id sign-in Cookie)" ALTERNATIVE
require sign-in "$(exec_id sign-in sign-in-forms)" ALTERNATIVE
require sign-in-forms "$(exec_id sign-in-forms 'Username Password Form')" REQUIRED
require gated-browser "$(exec_id gated-browser gate-soju)" CONDITIONAL

E=$(exec_id gate-soju 'Condition - client scope')
require gate-soju "$E" REQUIRED
kc create "authentication/executions/$E/config" -r $R -s alias=gate-soju-scope \
	-s config.client_scope=soju_roles -s config.negate=false
E=$(exec_id gate-soju 'Condition - user role')
require gate-soju "$E" REQUIRED
kc create "authentication/executions/$E/config" -r $R -s alias=gate-soju-role \
	-s config.condUserRole=soju.access -s config.negate=true
E=$(exec_id gate-soju 'Deny access')
require gate-soju "$E" REQUIRED
kc create "authentication/executions/$E/config" -r $R -s alias=gate-soju-deny \
	-s 'config.denyErrorMessage=soju access required'

kc update realms/$R -s browserFlow=gated-browser
echo "realm $R built"
