import { useCallback, useEffect, useState } from "react";
import { useAuth } from "@/lib/auth";
// `refetch` was never wired to a caller; the effect below covers the
// reload-on-session-change case on its own.

const HUB_API = "/api/hub";

export interface UserPermissions {
  hub_bits: number;
  top_position: number;
  is_admin: boolean;
  is_creator: boolean;
}

// Permission bit constants (must match backend)
export const P = {
  READ: 1,
  WRITE: 2,
  CONNECT: 4,
  SPEAK: 8,
  VIDEO: 16,
  MANAGE_CHANNEL: 32,
  ADMIN_CHANNEL: 64,
  CREATE_TEXT_CHANNELS: 128,
  CREATE_VOICE_CHANNELS: 256,
  EDIT_OTHER_CHANNELS: 512,
  MANAGE_MEMBERS: 1024,
  INVITE_PERMANENT: 2048,
  CREATE_TEMP_LINKS: 4096,
  MANAGE_ROLES: 8192,
  ALL: 16383,
} as const;

export const HUB_PERMISSION_LABELS: { bit: number; key: string; label: string }[] = [
  { bit: P.CREATE_TEXT_CHANNELS, key: "create_text", label: "Create text channels" },
  { bit: P.CREATE_VOICE_CHANNELS, key: "create_voice", label: "Create voice channels" },
  { bit: P.EDIT_OTHER_CHANNELS, key: "edit_channels", label: "Edit other channels" },
  { bit: P.MANAGE_MEMBERS, key: "manage_members", label: "Manage members" },
  { bit: P.INVITE_PERMANENT, key: "invite_permanent", label: "Invite permanent users" },
  { bit: P.CREATE_TEMP_LINKS, key: "create_temp_links", label: "Create temp links" },
  { bit: P.MANAGE_ROLES, key: "manage_roles", label: "Manage roles" },
];

export function hasBit(bits: number, bit: number): boolean {
  return (bits & bit) === bit;
}

export function usePermissions() {
  const { session } = useAuth();
  const [perms, setPerms] = useState<UserPermissions | null>(null);

  useEffect(() => {
    if (!session?.hubId || !session?.token) return;
    let cancelled = false;
    const hubId = session.hubId;
    const token = session.token;
    (async () => {
      try {
        const res = await fetch(
          `${HUB_API}/v1/hubs/${hubId}/my-permissions`,
          { headers: { Authorization: `Bearer ${token}` } },
        );
        if (cancelled || !res.ok) return;
        const data = (await res.json()) as UserPermissions;
        if (!cancelled) setPerms(data);
      } catch { /* ignore */ }
    })();
    return () => {
      cancelled = true;
    };
  }, [session?.hubId, session?.token]);

  const has = useCallback(
    (bit: number) => {
      if (!perms) return false;
      return perms.is_admin || (perms.hub_bits & bit) === bit;
    },
    [perms],
  );

  return { perms, has };
}
