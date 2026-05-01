// Single source of truth for the current hub's profile (name, slug,
// avatar, description). HubSwitcher and the General Settings dialog
// share this so a successful PATCH propagates to the sidebar without
// any prop wiring.

import useSWR from "swr";
import { useAuth } from "@/lib/auth";

const HUB_API = "/api/hub";

export interface Hub {
  id: string;
  name: string;
  slug: string;
  plan: string;
  avatar_url: string | null;
  description: string | null;
  creator_id: string | null;
  created_at: string;
  updated_at: string;
}

const hubKey = (hubId?: string) => (hubId ? ["hub", hubId] : null);

export function useHub() {
  const { session } = useAuth();

  const { data, isLoading, mutate } = useSWR<Hub>(
    hubKey(session?.hubId),
    async () => {
      const res = await fetch(`${HUB_API}/v1/hubs/${session!.hubId}`, {
        headers: { Authorization: `Bearer ${session!.token}` },
      });
      if (!res.ok) throw new Error(`GET hub: ${res.status}`);
      return res.json();
    },
    { revalidateOnFocus: false, dedupingInterval: 5000 },
  );

  return { hub: data ?? null, loading: isLoading, mutate };
}

export async function patchHub(
  hubId: string,
  token: string,
  body: { name?: string; description?: string | null },
): Promise<Hub | { error: string; status: number }> {
  const res = await fetch(`${HUB_API}/v1/hubs/${hubId}`, {
    method: "PATCH",
    headers: {
      "Content-Type": "application/json",
      Authorization: `Bearer ${token}`,
    },
    body: JSON.stringify(body),
  });
  if (res.status === 403) return { error: "forbidden", status: 403 };
  if (!res.ok) return { error: `status ${res.status}`, status: res.status };
  return res.json();
}

export async function uploadHubAvatar(
  hubId: string,
  token: string,
  file: File,
): Promise<{ url: string } | { error: string; status: number }> {
  const fd = new FormData();
  fd.append("file", file);
  const res = await fetch(`${HUB_API}/v1/hubs/${hubId}/avatar`, {
    method: "POST",
    headers: { Authorization: `Bearer ${token}` },
    body: fd,
  });
  if (res.status === 403) return { error: "forbidden", status: 403 };
  if (!res.ok) return { error: `status ${res.status}`, status: res.status };
  return res.json();
}
