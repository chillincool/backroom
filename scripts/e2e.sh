#!/usr/bin/env bash
# End-to-end tests against real soju, Keycloak and ergo in Docker Compose.
# Used by `mise run e2e*` and CI.
#
#   e2e.sh up     generate test certs, start the stack, create soju users
#   e2e.sh test   run the e2e tests against the running stack
#   e2e.sh down   stop the stack and delete its data
#   e2e.sh all    up + test + down (down also runs if something fails;
#                 set E2E_KEEP=1 to keep the stack for debugging)
#   e2e.sh realm  regenerate test/e2e/keycloak/realm.json (slow; not used by CI)
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
HERE=$ROOT/test/e2e
STATE=$ROOT/.e2e
COMPOSE=(docker compose -f "$HERE/compose.yaml")
KEYCLOAK_IMAGE=$(sed -n 's/^ *image: \(quay.io\/keycloak\/keycloak:.*\)$/\1/p' "$HERE/compose.yaml")

log() { printf '\n==> %s\n' "$*"; }

# wait_until SECONDS DESCRIPTION COMMAND...
wait_until() {
	local deadline=$((SECONDS + $1)) what=$2
	shift 2
	until "$@" > /dev/null 2>&1; do
		if ((SECONDS > deadline)); then
			echo "timed out waiting for $what" >&2
			return 1
		fi
		sleep 2
	done
}

tcp_open() { (exec 3<> "/dev/tcp/127.0.0.1/$1") 2> /dev/null; }

# A throwaway CA and a soju certificate for localhost, signed by it.
make_certs() {
	local dir=$STATE/certs
	[[ -f $dir/soju.crt ]] && return
	mkdir -p "$dir"
	openssl req -x509 -newkey rsa:2048 -nodes -days 30 -subj "/CN=backroom e2e CA" \
		-keyout "$dir/ca.key" -out "$dir/ca.crt" 2> /dev/null
	openssl req -newkey rsa:2048 -nodes -subj "/CN=localhost" \
		-keyout "$dir/soju.key" -out "$dir/soju.csr" 2> /dev/null
	printf 'subjectAltName=DNS:localhost,IP:127.0.0.1\n' > "$dir/soju.ext"
	openssl x509 -req -in "$dir/soju.csr" -CA "$dir/ca.crt" -CAkey "$dir/ca.key" -CAcreateserial \
		-days 30 -extfile "$dir/soju.ext" -out "$dir/soju.crt" 2> /dev/null
	# soju runs as root in its image, but keep the key readable for rootless setups.
	chmod 644 "$dir/soju.key"
}

up() {
	make_certs
	log "Starting Keycloak, ergo and the auth stub"
	"${COMPOSE[@]}" up -d --quiet-pull keycloak ergo httpauth
	# soju's `auth oauth2` does OIDC discovery at startup, so Keycloak first.
	log "Waiting for Keycloak and the e2e realm (slow on first start)"
	wait_until 900 "Keycloak" curl -sf http://127.0.0.1:18080/realms/e2e/.well-known/openid-configuration

	log "Starting soju (oauth2, internal, http)"
	"${COMPOSE[@]}" up -d --quiet-pull soju-oauth2 soju-internal soju-http
	for port in 18081 16667 16697 28081 26667 26697 38081 36667 36697; do
		wait_until 60 "port $port" tcp_open "$port"
	done

	log "Creating soju users"
	# Fails harmlessly when the user exists (re-running `up`).
	"${COMPOSE[@]}" exec -T soju-internal sojuctl -config /soju-config \
		user create -username dave -password dave-secret || true
}

test_() {
	log "Running e2e tests"
	BACKROOM_E2E_CA=$STATE/certs/ca.crt \
		cargo test -p backroom-core --test e2e -- --ignored --test-threads=4
}

down() {
	log "Stopping the stack"
	"${COMPOSE[@]}" down -v --remove-orphans
}

logs() {
	log "Service logs"
	"${COMPOSE[@]}" logs --no-color --tail 200 || true
}

all() {
	if [[ -z ${E2E_KEEP:-} ]]; then
		# On failure, print the service logs before they're deleted.
		trap 'status=$?; if ((status)); then logs; fi; down' EXIT
	fi
	up
	test_
}

realm() {
	local name=backroom-e2e-realm-build port=18090
	docker rm -f "$name" > /dev/null 2>&1 || true
	trap 'docker rm -f '"$name"' > /dev/null' EXIT
	log "Starting a scratch Keycloak ($KEYCLOAK_IMAGE)"
	docker run -d --name "$name" -p "127.0.0.1:$port:8080" \
		-e KC_BOOTSTRAP_ADMIN_USERNAME=admin -e KC_BOOTSTRAP_ADMIN_PASSWORD=admin \
		"$KEYCLOAK_IMAGE" start-dev > /dev/null
	wait_until 900 "Keycloak" curl -sf "http://127.0.0.1:$port/realms/master"
	log "Building the realm"
	"$HERE/keycloak/build-realm.sh" "$name"
	log "Exporting"
	local token
	token=$(curl -sf -d client_id=admin-cli -d username=admin -d password=admin -d grant_type=password \
		"http://127.0.0.1:$port/realms/master/protocol/openid-connect/token" |
		python3 -I -c 'import json, sys; print(json.load(sys.stdin)["access_token"])')
	mkdir -p "$STATE"
	curl -sf -X POST -H "Authorization: Bearer $token" -o "$STATE/partial.json" \
		"http://127.0.0.1:$port/admin/realms/e2e/partial-export?exportClients=true&exportGroupsAndRoles=true"
	python3 -I "$HERE/keycloak/finish-export.py" "$STATE/partial.json" "$HERE/keycloak/realm.json"
	echo "wrote test/e2e/keycloak/realm.json"
}

case ${1:-} in
up) up ;;
test) test_ ;;
down) down ;;
all) all ;;
realm) realm ;;
*)
	sed -n '2,10p' "$0" >&2
	exit 2
	;;
esac
