import { useMemo, useSyncExternalStore } from "react";
import {
  getCachedMe,
  getTokenMode,
  subscribeAuthChange,
  type MeResponse,
  type Permission,
} from "./api";

export function hasPerm(
  me: MeResponse | null,
  required: Permission
): boolean {
  if (!me) {
    return getTokenMode() === "dev_static";
  }
  if (Array.isArray(me.roles) && me.roles.includes("ADMIN")) {
    return true;
  }
  return Array.isArray(me.permissions) && me.permissions.includes(required);
}

export function hasAnyPerm(
  me: MeResponse | null,
  requiredList: readonly Permission[]
): boolean {
  if (requiredList.length === 0) {
    return true;
  }
  if (!me) {
    return getTokenMode() === "dev_static";
  }
  if (Array.isArray(me.roles) && me.roles.includes("ADMIN")) {
    return true;
  }
  if (!Array.isArray(me.permissions)) {
    return false;
  }
  return requiredList.some((p) => me.permissions.includes(p));
}

export function usePrincipal(): {
  readonly me: MeResponse | null;
  readonly can: (perm: Permission) => boolean;
  readonly canAny: (perms: readonly Permission[]) => boolean;
  readonly isAdmin: boolean;
} {
  const me = useSyncExternalStore(
    subscribeAuthChange,
    getCachedMe,
    () => null
  );
  return useMemo(() => {
    const isAdmin = Boolean(me?.roles?.includes("ADMIN"));
    return {
      me,
      can: (perm: Permission) => hasPerm(me, perm),
      canAny: (perms: readonly Permission[]) => hasAnyPerm(me, perms),
      isAdmin,
    };
  }, [me]);
}
