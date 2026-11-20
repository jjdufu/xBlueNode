#!/usr/bin/env bash
set -euo pipefail
source "$(dirname "$0")/../gate/lib.sh"

# Seed Step 11 xBlueView 3D twin asset, bindings, and published supply-overview dashboard (`10-STEP11-12` §11.8).
# Idempotent: reuses existing asset and dashboard by name/slug if already present.

GLB_PATH="$ROOT/axpoint-parser/tests/data/models/warehouse_twin.glb"
if [[ ! -f "$GLB_PATH" ]]; then
  (cd "$ROOT/axpoint-parser" && uv run python tests/fixtures/make_fixtures.py >/dev/null 2>&1 || python3 tests/fixtures/make_fixtures.py >/dev/null)
fi

WAREHOUSE_NODE_ID="${WAREHOUSE_NODE_ID:-$(sql_app default "SELECT id FROM xb_nodes WHERE node_key = 'W-1' LIMIT 1")}"
SIM_S1_ID="${SIM_S1_ID:-$(api GET "/api/axsim/sandboxes/${SANDBOX_ID:-}/scenarios" 2>/dev/null | jq -r '.[0].id // empty')}"
SIM_S2_ID="${SIM_S2_ID:-$(api GET "/api/axsim/sandboxes/${SANDBOX_ID:-}/scenarios" 2>/dev/null | jq -r '.[1].id // empty')}"

# 1. Upload or reuse warehouse_twin.glb (MODEL_3D) and bind W-1 to mesh Rack_A.
MODEL3D_ASSET_ID=$(api GET "/api/xblueview/assets?asset_kind=MODEL_3D" | jq -r '.items[] | select(.name=="warehouse_twin.glb") | .id' | head -n 1)
if [[ -z "$MODEL3D_ASSET_ID" ]]; then
  MODEL3D_ASSET_ID=$(curl -sS -X POST \
    -H "Authorization: Bearer $AXPOINT_DEV_TOKEN" \
    -F "asset_kind=MODEL_3D" \
    -F "name=warehouse_twin.glb" \
    -F "file=@$GLB_PATH;type=model/gltf-binary" \
    "$CORE_BASE_URL/api/xblueview/assets" | jq -r '.id')
fi

if [[ -n "$WAREHOUSE_NODE_ID" ]]; then
  api PUT "/api/xblueview/assets/$MODEL3D_ASSET_ID/bindings3d" \
    "{\"bindings\":[{\"node_id\":\"$WAREHOUSE_NODE_ID\",\"mesh_name\":\"Rack_A\",\"pos_x\":0,\"pos_y\":0,\"pos_z\":0,\"scale\":1.0}]}" >/dev/null
fi

# 2. Create or reuse published supply-overview dashboard with 8 widgets.
EXISTING=$(api GET "/api/xblueview/dashboards/by-slug/supply-overview" || true)
DASHBOARD_ID=$(printf '%s' "$EXISTING" | jq -r '.id // empty' 2>/dev/null || true)

if [[ -z "$DASHBOARD_ID" ]]; then
  DASHBOARD_ID=$(api POST /api/xblueview/dashboards '{
    "name":"供应链全景驾驶舱",
    "slug":"supply-overview",
    "description":"华东供应链指标、下钻分析、三维数字孪生与回写闭环驾驶舱",
    "theme":"light",
    "canvas":{"cols":24,"row_height":30,"screen_width":1920,"screen_height":1080},
    "filters":[
      {"key":"status","label":"订单状态","kind":"SELECT","field_map":{}}
    ]
  }' | jq -r '.id')

  KPI_WIDGET_ID=$(api POST "/api/xblueview/dashboards/$DASHBOARD_ID/widgets" '{
    "widget_kind":"KPI_CARD",
    "title":"已支付金额",
    "position":{"x":0,"y":0,"w":6,"h":4},
    "data_source":{"kind":"METRIC","metric":"paid_amount","dimensions":[]}
  }' | jq -r '.id')

  BAR_WIDGET_ID=$(api POST "/api/xblueview/dashboards/$DASHBOARD_ID/widgets" '{
    "widget_kind":"BAR_CHART",
    "title":"按状态订单金额",
    "position":{"x":6,"y":0,"w":10,"h":6},
    "data_source":{"kind":"SQL","sql":"SELECT status, warehouse_code, sum(amount) AS total_amount FROM lake.demo_orders GROUP BY status, warehouse_code"},
    "encoding":{"x":"status","y":["total_amount"]},
    "drill":[{"field":"status"},{"field":"warehouse_code","node_type":"Warehouse","node_key_field":"warehouse_code"}]
  }' | jq -r '.id')

  api POST "/api/xblueview/dashboards/$DASHBOARD_ID/widgets" '{
    "widget_kind":"LINE_CHART",
    "title":"仓库库存趋势",
    "position":{"x":16,"y":0,"w":8,"h":6},
    "data_source":{"kind":"NODE_METRIC","type_name":"Warehouse","metric":"inventory","agg":"AVG","grain":"HOUR","time_range":"LAST_7_DAYS","group_by":"time"}
  }' >/dev/null

  api POST "/api/xblueview/dashboards/$DASHBOARD_ID/widgets" '{
    "widget_kind":"MAP",
    "title":"供应链地理态势",
    "position":{"x":0,"y":4,"w":6,"h":6},
    "data_source":{"kind":"NODES_GEO","type_names":["Factory","Warehouse"],"color_metric":"inventory"}
  }' >/dev/null

  SCENE3D_WIDGET_ID=$(api POST "/api/xblueview/dashboards/$DASHBOARD_ID/widgets" "{
    \"widget_kind\":\"SCENE_3D\",
    \"title\":\"仓库三维孪生\",
    \"position\":{\"x\":6,\"y\":6,\"w\":10,\"h\":8},
    \"data_source\":{\"kind\":\"SCENE_3D\",\"asset_id\":\"$MODEL3D_ASSET_ID\",\"color_metric\":\"inventory\"}
  }" | jq -r '.id')

  api POST "/api/xblueview/dashboards/$DASHBOARD_ID/widgets" '{
    "widget_kind":"ALERT_LIST",
    "title":"运营告警列表",
    "position":{"x":16,"y":6,"w":8,"h":4},
    "data_source":{"kind":"ALERTS","status":["OPEN","ACK"],"limit":20}
  }' >/dev/null

  SCENARIO_ARRAY="[]"
  if [[ -n "$SIM_S1_ID" && -n "$SIM_S2_ID" ]]; then
    SCENARIO_ARRAY="[\"$SIM_S1_ID\",\"$SIM_S2_ID\"]"
  elif [[ -n "$SIM_S1_ID" ]]; then
    SCENARIO_ARRAY="[\"$SIM_S1_ID\"]"
  fi
  api POST "/api/xblueview/dashboards/$DASHBOARD_ID/widgets" "{
    \"widget_kind\":\"SIM_COMPARE\",
    \"title\":\"推演方案对比\",
    \"position\":{\"x\":0,\"y\":10,\"w\":6,\"h\":4},
    \"data_source\":{\"kind\":\"SIM_SCENARIOS\",\"scenario_ids\":$SCENARIO_ARRAY,\"metrics\":[\"fulfillment\"]}
  }" >/dev/null

  ACTION_WIDGET_ID=$(api POST "/api/xblueview/dashboards/$DASHBOARD_ID/widgets" "{
    \"widget_kind\":\"ACTION_FORM\",
    \"title\":\"库存盘点回写\",
    \"position\":{\"x\":16,\"y\":10,\"w\":8,\"h\":4},
    \"data_source\":{\"kind\":\"ACTION\",\"action_type\":\"WRITE_NODE\",\"node_id\":\"$WAREHOUSE_NODE_ID\",\"target_kind\":\"METRIC\",\"key\":\"inventory\",\"input_schema\":{\"type\":\"object\",\"required\":[\"value\"],\"properties\":{\"value\":{\"type\":\"number\"}}}}
  }" | jq -r '.id')

  api POST "/api/xblueview/dashboards/$DASHBOARD_ID/publish" '{}' >/dev/null
else
  KPI_WIDGET_ID=$(printf '%s' "$EXISTING" | jq -r '.widgets[] | select(.widget_kind=="KPI_CARD") | .id' | head -n 1)
  BAR_WIDGET_ID=$(printf '%s' "$EXISTING" | jq -r '.widgets[] | select(.widget_kind=="BAR_CHART") | .id' | head -n 1)
  SCENE3D_WIDGET_ID=$(printf '%s' "$EXISTING" | jq -r '.widgets[] | select(.widget_kind=="SCENE_3D") | .id' | head -n 1)
  ACTION_WIDGET_ID=$(printf '%s' "$EXISTING" | jq -r '.widgets[] | select(.widget_kind=="ACTION_FORM") | .id' | head -n 1)
fi

echo "MODEL3D_ASSET_ID=$MODEL3D_ASSET_ID"
echo "DASHBOARD_ID=$DASHBOARD_ID"
echo "KPI_WIDGET_ID=$KPI_WIDGET_ID"
echo "BAR_WIDGET_ID=$BAR_WIDGET_ID"
echo "SCENE3D_WIDGET_ID=$SCENE3D_WIDGET_ID"
echo "ACTION_WIDGET_ID=$ACTION_WIDGET_ID"
