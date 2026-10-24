"""Turns a Keycloak partial export of the e2e realm into realm.json.

Partial exports mask secrets and include signing keys (masked too), and
leave out users. This restores the soju test secret, drops the keys so
Keycloak generates new ones on import, and adds the test users.

    python3 finish-export.py partial.json realm.json
"""

import json
import sys

realm = json.load(open(sys.argv[1]))

for client in realm["clients"]:
    if client["clientId"] == "soju":
        client["secret"] = "soju-e2e-secret"
realm.get("components", {}).pop("org.keycloak.keys.KeyProvider", None)
if "**********" in json.dumps(realm):
    sys.exit("a masked secret is still in the export")


def user(name, groups):
    # Password = username.
    return {
        "username": name,
        "enabled": True,
        "email": f"{name}@e2e.test",
        "emailVerified": True,
        "firstName": name,
        "lastName": "E2E",
        "groups": groups,
        "credentials": [{"type": "password", "value": name, "temporary": False}],
    }


realm["users"] = [user("alice", ["/soju-users"]), user("mallory", [])]

with open(sys.argv[2], "w") as f:
    json.dump(realm, f, indent=2, sort_keys=True)
    f.write("\n")
