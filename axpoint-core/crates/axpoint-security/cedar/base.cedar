// 1. 具备权限、资源属于本部门（或本部门的下级部门）、密级足够
permit (principal, action, resource)
when { principal.permissions.contains(context.action)
       && principal in resource.owner_dept
       && principal.clearance >= resource.sensitivity };

// 2. 公开与内部资源：具备权限即可跨部门访问
permit (principal, action, resource)
when { principal.permissions.contains(context.action) && resource.sensitivity <= 1 };

// 3. 机密及以上资源跨部门访问：需要包含本次动作的生效授权（经属主部门审批，视同密级满足）
permit (principal, action, resource)
when { principal.permissions.contains(context.action)
       && resource.granted.containsAny(principal.grant_keys) };

// 4. 平台管理动作不受资源归属约束
permit (principal, action in [AxPoint::Action::"GUARD_ADMIN", AxPoint::Action::"OPS_ADMIN"], resource)
when { principal.permissions.contains(context.action) };

// 5. 数据管理员与安全管理员对全部数据资源执行其具备的动作（与行策略对 DATA_ADMIN / GUARD_ADMIN 的豁免一致）
permit (principal, action, resource)
when { (principal.permissions.contains("DATA_ADMIN") || principal.permissions.contains("GUARD_ADMIN"))
       && principal.permissions.contains(context.action) };
