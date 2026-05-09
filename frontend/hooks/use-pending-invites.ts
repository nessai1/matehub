// Roster's quiet sibling: people who've been invited but haven't shown up
// yet. The Rust side (`GET /v1/hubs/{hub_id}/pending-invites`) merges three
// stores — invitations.used_at IS NULL, temp_users.active_session IS NULL,
// and active invite_links — into one list, freshest first.
//
// Cache is invalidated by:
//   * usePresence on `member_joined` (an accept removes one of these rows)
//   * the Add Teammates dialog after a successful invite create

import useSWR, { mutate as globalMutate } from "swr";
import { useAuth } from "@/lib/auth";

const HUB_API = "/api/hub";

export interface PendingInvite {
  kind: "permanent" | "temp" | "link";
  id: string;
  /// For permanent: pre-allocated username. For temp: nickname. For link:
  /// empty string — the FE renders a counter instead.
  name: string;
  email: string | null;
  expires_at: string | null;
  created_at: string;
  group_id: string | null;
  created_by: string;
  created_by_name: string;
  created_by_username: string;
  /// Invite-link cap. null for `permanent` and `temp`.
  max_uses: number | null;
  /// Invite-link counter. null for non-link kinds.
  uses_count: number | null;
}

export function pendingInvitesKey(hubId: string | undefined) {
  return hubId ? (["pending-invites", hubId] as const) : null;
}

async function fetchPending(
  hubId: string,
  token: string | undefined,
): Promise<PendingInvite[]> {
  if (!token) return [];
  const res = await fetch(`${HUB_API}/v1/hubs/${hubId}/pending-invites`, {
    headers: { Authorization: `Bearer ${token}` },
  });
  if (res.status === 401) {
    window.location.href = "/login";
    return [];
  }
  if (!res.ok) throw new Error(`pending-invites ${res.status}`);
  return (await res.json()) as PendingInvite[];
}

export function usePendingInvites() {
  const { session } = useAuth();

  const { data, isLoading, mutate } = useSWR<PendingInvite[]>(
    pendingInvitesKey(session?.hubId),
    ([, hubId]: readonly ["pending-invites", string]) =>
      fetchPending(hubId, session?.token),
    {
      revalidateOnFocus: false,
      revalidateOnReconnect: true,
      dedupingInterval: 2000,
    },
  );

  return {
    invites: data ?? [],
    loading: isLoading,
    refetch: mutate,
  };
}

/** Trigger a refetch of pending-invites for a given hub from anywhere. */
export function invalidatePendingInvites(hubId: string | undefined) {
  if (!hubId) return;
  globalMutate(pendingInvitesKey(hubId));
}

/** DELETE a pending invite. Returns the response status — 204 = ok,
 *  403 = no perm, 404 = vanished already, 410 = accepted/used in the gap. */
export async function deletePendingInvite(
  hubId: string,
  token: string,
  invite: Pick<PendingInvite, "kind" | "id">,
): Promise<number> {
  const res = await fetch(
    `${HUB_API}/v1/hubs/${hubId}/pending-invites/${invite.kind}/${invite.id}`,
    {
      method: "DELETE",
      headers: { Authorization: `Bearer ${token}` },
    },
  );
  if (res.ok || res.status === 404 || res.status === 410) {
    // Either we removed it or it was already gone — refresh either way so
    // the sidebar matches reality.
    invalidatePendingInvites(hubId);
  }
  return res.status;
}
