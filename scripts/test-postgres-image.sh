#!/bin/sh
# Disposable Docker DB only. Never reads .env or contacts the user's database.
set -eu
image=${1:?Usage: sh scripts/test-postgres-image.sh IMAGE}
default_env=$(docker image inspect --format '{{range .Config.Env}}{{println .}}{{end}}' "$image")
printf '%s\n' "$default_env" | grep -Fqx 'REPO_BACKEND=postgres'
printf '%s\n' "$default_env" | grep -Fqx 'DATABASE_URL='
printf '%s\n' "$default_env" | grep -Fqx 'POSTGRES_AUTO_MIGRATE=true'
printf '%s\n' "$default_env" | grep -Fqx 'PORT=9091'
if printf '%s\n' "$default_env" | grep -q '^DATABASE_PASSWORD='; then
    echo 'Refusing to test image that contains a database password'
    exit 1
fi
test_id="akun-image-${CI_JOB_ID:-$$}"
network="$test_id-net"
db="$test_id-db"
api="$test_id-api"
cleanup() {
    docker rm -fv "$api" "$db" >/dev/null 2>&1 || true
    docker network rm "$network" >/dev/null 2>&1 || true
}
trap cleanup EXIT
trap 'exit 1' HUP INT TERM
docker network create "$network" >/dev/null
docker run -d --name "$db" --network "$network" \
    -e POSTGRES_DB=smoke -e POSTGRES_USER=smoke \
    -e POSTGRES_PASSWORD=isolated_smoke_password postgres:17-alpine >/dev/null
attempt=0
until docker exec "$db" pg_isready -U smoke -d smoke >/dev/null 2>&1; do
    attempt=$((attempt + 1))
    if [ "$attempt" -ge 60 ]; then echo 'PostgreSQL failed to start'; exit 1; fi
    sleep 1
done
docker run -d --name "$api" --network "$network" \
    -e "DATABASE_URL=postgres://smoke:isolated_smoke_password@$db:5432/smoke" \
    -e 'POSTGRES_BOOTSTRAP_USER=smoke@example.test|isolated_app_password' \
    -e PORT=8080 -e SERVER_PORT=9091 \
    -e CORS_ALLOWED_ORIGINS=https://frontend.example.test "$image" >/dev/null
attempt=0
until docker exec "$api" curl --fail --silent --max-time 4 \
    http://localhost:8080/health/ready >/dev/null 2>&1; do
    attempt=$((attempt + 1))
    if [ "$attempt" -ge 60 ]; then echo 'API readiness failed'; exit 1; fi
    sleep 1
done
expect_status() {
    expected=$1
    shift
    actual=$(docker exec "$api" curl --silent --max-time 10 --output /dev/null --write-out '%{http_code}' "$@")
    if [ "$actual" != "$expected" ]; then
        echo "API smoke: expected HTTP $expected, got $actual"
        exit 1
    fi
}
for login in login loginv2; do
    expect_status 200 -X POST --data 'email=smoke%40example.test&password=isolated_app_password' \
        "http://localhost:8080/api/v1/user/$login"
done
token=$(docker exec "$api" curl --fail --silent --max-time 10 -X POST \
    --data 'email=smoke%40example.test&password=isolated_app_password' \
    http://localhost:8080/api/v1/user/loginv2 | sed -n 's/.*"token":"\([^"]*\)".*/\1/p')
test -n "$token"
users=$(docker exec "$api" curl --fail --silent --max-time 10 \
    -H "Authorization: Bearer $token" http://localhost:8080/api/v2/user)
printf '%s' "$users" | grep -q '^\['
printf '%s' "$users" | grep -q '"lifecycleEntity"'
printf '%s' "$users" | grep -q '"permissionProfiles"'
for endpoint in fetchUserRoles getAllPermissionFunctions getAuthLevels getAccessLevels fetchAuthorizedPermissionProfiles; do
    expect_status 200 -H "Authorization: Bearer $token" "http://localhost:8080/api/v2/user/$endpoint"
done
expect_status 200 -X POST -H "Authorization: Bearer $token" \
    --data 'shortname=SMOKE&name=Smoke&activationStatus=Active&userRoleFunctions[0].function.identFunction=1&userRoleFunctions[0].accessLevel.identAccessLevel=2' \
    http://localhost:8080/api/v2/user/create-user-role
expect_status 400 -X POST -H "Authorization: Bearer $token" \
    --data 'shortname=INVALID&activationStatus=invalid' http://localhost:8080/api/v2/user/create-user-role
expect_status 400 -X POST --data 'email=smoke%40example.test&password=wrong' http://localhost:8080/api/v1/user/loginv2
expect_status 403 http://localhost:8080/api/v2/user
expect_status 401 -H 'Authorization: Bearer invalid' http://localhost:8080/api/v2/user
expect_status 200 -X OPTIONS -H 'Origin: https://frontend.example.test' \
    -H 'Access-Control-Request-Method: PUT' \
    -H 'Access-Control-Request-Headers: authorization,content-type' http://localhost:8080/api/v2/user
docker exec "$api" sh -ec 'test "$(id -u)" != 0; test ! -d /opt/oracle'
# A restart must reuse the migrated schema and retain bootstrap credentials.
docker restart "$api" >/dev/null
attempt=0
until docker exec "$api" curl --fail --silent --max-time 4 \
    http://localhost:8080/health/ready >/dev/null 2>&1; do
    attempt=$((attempt + 1))
    if [ "$attempt" -ge 60 ]; then echo 'API restart readiness failed'; exit 1; fi
    sleep 1
done
expect_status 200 -X POST --data 'email=smoke%40example.test&password=isolated_app_password' http://localhost:8080/api/v1/user/loginv2
echo 'PostgreSQL image smoke passed: migrations, readiness, PORT override, v1/v2 login, auth, user DTOs, role/profile endpoints, form validation, CORS preflight, non-root, restart.'
