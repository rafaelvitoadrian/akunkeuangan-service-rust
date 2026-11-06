#!/usr/bin/env bash
# Rekam ulang SELURUH golden fixture dari sistem lama yang berjalan.
#
# Pemakaian:
#   BASE=http://localhost:9090 EMAIL=demo@example.test PASSWORD=<your-password> ./scripts/record-golden.sh
#   BASE=... EMAIL=demo@example.test PASSWORD=<your-password> LOGIN_PATH=/api/v1/user/login ./scripts/record-golden.sh
#
# Hanya membaca data milik user yang login (tidak menyentuh user lain).

set -uo pipefail

BASE="${BASE:-http://localhost:9090}"
EMAIL="${EMAIL:?Set EMAIL to your test user}"
PASSWORD="${PASSWORD:?Set PASSWORD explicitly}"
LOGIN_PATH="${LOGIN_PATH:-/api/v1/user/loginv2}"
OUT="$(cd "$(dirname "$0")/.." && pwd)/golden"
mkdir -p "$OUT/json" "$OUT/headers"

rec() {
  local name="$1"; shift
  local code
  code=$(curl -s -m 20 -o "$OUT/json/$name.json" -D "$OUT/headers/$name.txt" -w '%{http_code}' "$@")
  echo "$code" > "$OUT/headers/$name.status"
  printf "  %-30s %s  %sB\n" "$name" "$code" "$(wc -c < "$OUT/json/$name.json")"
}

echo "== login =="
TOKEN=$(curl -s -m 15 -X POST "$BASE$LOGIN_PATH?email=$EMAIL&password=$PASSWORD" \
  | python3 -c 'import json,sys; print(json.load(sys.stdin)["token"])') || exit 2
[ -n "$TOKEN" ] || { echo "login gagal"; exit 2; }
A=(-H "Authorization: Bearer $TOKEN")
echo "  token OK (${#TOKEN} char)"

echo "== id data milik $EMAIL =="
AKUN_ID=$(curl -s "${A[@]}" "$BASE/api/v1/akunkeuangan" \
  | python3 -c 'import json,sys; d=json.load(sys.stdin); print(d["data"][0]["id"] if d.get("data") else "")')
USER_ID=$(curl -s "${A[@]}" "$BASE/api/v2/user" \
  | python3 -c "import json,sys; d=json.load(sys.stdin); print(next((u['id'] for u in d['data'] if u.get('email')=='$EMAIL'), ''))")
CAT_ID=""; TK_ID=""
if [ -n "$AKUN_ID" ]; then
  CAT_ID=$(curl -s "${A[@]}" "$BASE/api/v1/category/lists?idAkun=$AKUN_ID" \
    | python3 -c 'import json,sys; d=json.load(sys.stdin); print(d["data"][0]["id"] if d.get("data") else "")')
  TK_ID=$(curl -s "${A[@]}" "$BASE/api/v1/transaksikeuangan/lists?idAkun=$AKUN_ID" \
    | python3 -c 'import json,sys; d=json.load(sys.stdin); print(d["data"][0]["id"] if d.get("data") else "")')
fi
echo "  akun=$AKUN_ID user=$USER_ID category=$CAT_ID transaksi=$TK_ID"

echo "== permitAll & error =="
rec login_401 -X POST "$BASE/api/v1/user/login?email=$EMAIL&password=salah"
rec loginv2_401 -X POST "$BASE/api/v1/user/loginv2?email=$EMAIL&password=salah"
rec login_missing_params -X POST "$BASE/api/v1/user/login"
rec method_not_allowed -X GET "$BASE/api/v1/user/login"

echo "== tanpa token =="
rec users_no_token "$BASE/api/v2/user"
rec akun_list_no_token "$BASE/api/v1/akunkeuangan"
rec unknown_path "$BASE/api/v1/doesnotexist"
rec integration_no_token "$BASE/api/v1/integration"

echo "== token invalid =="
rec users_bad_token -H "Authorization: Bearer abc.def.ghi" "$BASE/api/v2/user"

echo "== dengan token =="
rec users_list "${A[@]}" "$BASE/api/v2/user"
[ -n "$USER_ID" ] && rec user_detail "${A[@]}" "$BASE/api/v2/user/$USER_ID"
rec akun_list "${A[@]}" "$BASE/api/v1/akunkeuangan"
[ -n "$AKUN_ID" ] && rec akun_detail "${A[@]}" "$BASE/api/v1/akunkeuangan/$AKUN_ID"
rec akun_test "${A[@]}" "$BASE/api/v1/akunkeuangan/test"
[ -n "$AKUN_ID" ] && rec category_lists "${A[@]}" "$BASE/api/v1/category/lists?idAkun=$AKUN_ID"
[ -n "$CAT_ID" ] && rec category_detail "${A[@]}" "$BASE/api/v1/category/detail?idAkun=$AKUN_ID&idC=$CAT_ID"
[ -n "$AKUN_ID" ] && rec tk_lists "${A[@]}" "$BASE/api/v1/transaksikeuangan/lists?idAkun=$AKUN_ID"
[ -n "$TK_ID" ] && rec tk_detail "${A[@]}" "$BASE/api/v1/transaksikeuangan/detail?idAkun=$AKUN_ID&idTk=$TK_ID"
rec event_entries "${A[@]}" "$BASE/event/evententries?eventId=nonexistent"
rec category_lists_missing_id "${A[@]}" "$BASE/api/v1/category/lists"
rec category_lists_bad_id "${A[@]}" "$BASE/api/v1/category/lists?idAkun=abc"
rec tk_lists_missing_id "${A[@]}" "$BASE/api/v1/transaksikeuangan/lists"

echo "== endpoint yang rusak di sistem lama (perilaku dipertahankan) =="
rec csvstreamer "${A[@]}" "$BASE/csvstreamer/successfuluploads"
rec admin "${A[@]}" "$BASE/admin"

echo "Selesai → $OUT"
