"use client";

import { useCallback, useEffect, useState } from "react";
import { useAuth } from "@/lib/auth";

const HUB_API = process.env.NEXT_PUBLIC_HUB_API_URL || "http://localhost:3002";

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

  const fetchPerms = useCallback(async () => {
    if (!session?.hubId || !session?.token) return;
    try {
      const res = await fetch(
        `${HUB_API}/v1/hubs/${session.hubId}/my-permissions`,
        { headers: { Authorization: `Bearer ${session.token}` } },
      );
      if (res.ok) setPerms(await res.json());
    } catch { /* ignore */ }
  }, [session?.hubId, session?.token]);

  useEffect(() => { fetchPerms(); }, [fetchPerms]);

  const has = useCallback(
    (bit: number) => {
      if (!perms) return false;
      return perms.is_admin || (perms.hub_bits & bit) === bit;
    },
    [perms],
  );

  return { perms, has, refetch: fetchPerms };
}
