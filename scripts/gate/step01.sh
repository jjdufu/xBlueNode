#!/usr/bin/env bash
set -euo pipefail
source "$(dirname "$0")/lib.sh"
cd "$ROOT/axpoint-parser"
pytest -q tests/test_security.py tests/test_profiler.py tests/test_csv.py tests/test_text.py tests/test_excel.py \
          tests/test_pptx.py tests/test_docx.py tests/test_pdf.py tests/test_json.py tests/test_xml.py tests/test_logs.py
cd "$ROOT"

parse() { curl -s -H "X-AxPoint-Internal-Token: $AXPOINT_INTERNAL_TOKEN" -H "X-AxPoint-Tenant: default" \
  -F "file=@axpoint-parser/tests/fixtures/$1" "$PARSER_BASE_URL/parse/unified"; }

expect_json "xlsx-blocks" "$(parse supply_chain.xlsx)" '(.tables | length) == 4 and (.tables | map(.table_name) | index("ku_cun_2") != null)'
expect_json "xlsx-types"  "$(parse supply_chain.xlsx)" '[.tables[] | select(.origin | startswith("sheet=订单")) | .columns[] | select(.name=="数量_实际") | .data_type][0] == "INT64"'
expect_json "csv-pii"     "$(parse orders.csv)" '[.tables[0].columns[] | select(.name=="phone") | .pii_category][0] == "PHONE"'
expect_json "pptx-notes"  "$(parse briefing.pptx)" '(.markdown | contains("备注")) and ([.tables[] | select(.origin | contains("chart="))] | length) == 1'
expect_json "docx-tables" "$(parse contract.docx)" '(.tables | length) == 2'
expect_json "json-child"  "$(parse api_orders.json)" '[.tables[].table_name] | index("api_orders__lines") != null'
expect_json "xml-records" "$(parse catalog.xml)" '.tables[0].row_count == 100'
expect_json "log-ts"      "$(parse app_java.log)" '[.tables[0].columns[] | select(.name=="ts") | .data_type][0] == "TIMESTAMP"'
expect_eq   "xxe-blocked" "$(curl -s -o /dev/null -w '%{http_code}' -H "X-AxPoint-Internal-Token: $AXPOINT_INTERNAL_TOKEN" \
  -H "X-AxPoint-Tenant: default" -F file=@axpoint-parser/tests/fixtures/xxe.xml "$PARSER_BASE_URL/parse/unified")" "422"
expect_eq   "staging-under-root" "$(parse orders.csv | jq -r '.tables[0].staging_path' | awk '/^tenants\/default\/staging\//{c++} END{print c+0}')" "1"

gate_pass step01
