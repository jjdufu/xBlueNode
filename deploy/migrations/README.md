# 数据库迁移规范

- 格式：sqlx，文件名 `NNNN_<module>.sql`，四位递增编号，只追加不修改。清单见 `docs/standards/A2-DATABASE-MIGRATIONS-INDEX.md`。
- 执行：`sqlx migrate run --source deploy/migrations --database-url "$MIGRATION_DATABASE_URL"`，或 Core 启动时自动执行（`AXPOINT_MIGRATE_ON_START=true`）。
- 已有表只用 `ALTER TABLE ... ADD COLUMN IF NOT EXISTS` 扩展；新表使用 `CREATE TABLE IF NOT EXISTS`。
- 每个带 `tenant_id` 的新表在迁移末尾执行 `SELECT ax_enable_tenant_rls('<table>');`。
- 检查：`sqlx migrate info --source deploy/migrations --database-url "$MIGRATION_DATABASE_URL"` 中不得出现 `pending`。
