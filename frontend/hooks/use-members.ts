import { useEffect, useRef } from "react";
import useSWR, { mutate as globalMutate } from "swr";
import { useAuth } from "@/lib/auth";
import {
  seedVoiceOccupancy,
  setVoiceOccupancy,
} from "@/lib/voice-occupancy-store";
import {
  clearVoiceMute,
  seedVoiceMute,
  setVoiceMute,
} from "@/lib/voice-mute-store";
import { pendingInvitesKey } from "@/hooks/use-pending-invites";

const HUB_API = "/api/hub";

export interface MemberGroup {
  id: string;
  name: string;
  color: string | null;
}

export interface Member {
  user_id: string;
  username: string;
  display_name: string;
  avatar_url: string | null;
  is_online: boolean;
  last_seen_at: string | null;
  groups: MemberGroup[];
  user_type: "permanent" | "temp";
  /** For temp users: when their magic-link dies. NULL for permanent users. */
  expires_at: string | null;
  /** Set if the account was scrubbed. NULL = alive. */
  deleted_at: string | null;
  /** Voice channel the member is currently in (null = not in voice). */
  current_voice_channel_id: string | null;
  /** Audio mute state when the member is in a voice channel. Hub-wide
   *  live updates flow through the voice-mute-store; this is the cold
   *  start value the server returns from /members-full. Default true
   *  for users not in voice — matches the video service Participant
   *  defaults so UI doesn't briefly mis-render. */
  voice_audio_muted: boolean;
  voice_video_muted: boolean;
}

/** True if the member should appear in the live roster (sidebar) and be
 *  pingable. Inactive members stay in the lookup map so chat history can
 *  still resolve their display_name — they just don't show up as people
 *  you can interact with. */
export function isMemberActive(m: Member): boolean {
  if (m.deleted_at) return false;
  if (m.expires_at && new Date(m.expires_at) <= new Date()) return false;
  return true;
}

/** SWR cache key. Exported so usePresence can target it via mutate(). */
export function membersKey(hubId: string | undefined) {
  return hubId ? (["members", hubId] as const) : null;
}

async function fetchMembers([, hubId]: readonly [
  "members",
  string,
]): Promise<Member[]> {
  const { token } = currentSession();
  if (!token) return [];

  const res = await fetch(`${HUB_API}/v1/hubs/${hubId}/members-full`, {
    headers: { Authorization: `Bearer ${token}` },
  });
  if (res.status === 401) {
    window.location.href = "/login";
    return [];
  }
  if (!res.ok) throw new Error(`members-full ${res.status}`);
  return (await res.json()) as Member[];
}

// SWR's fetcher receives the key but not the auth context. We read it from a
// module-level slot that useMembers keeps in sync below — cleaner than passing
// the token through the cache key (where it would needlessly partition the
// cache per-token rotation).
let _session: { token?: string } = {};
function currentSession() {
  return _session;
}

export function useMembers() {
  const { session } = useAuth();
  _session = session ?? {};

  const { data, isLoading, mutate } = useSWR<Member[]>(
    membersKey(session?.hubId),
    fetchMembers,
    {
      // No polling. Live updates come via the presence WS (member_joined,
      // member_left, member_groups_changed → see usePresence below) which
      // calls mutate() on the same key.
      revalidateOnFocus: false,
      revalidateOnReconnect: true,
      dedupingInterval: 2000,
      onSuccess: (members) => {
        seedVoiceOccupancy(
          members.map(
            (m) =>
              [m.user_id, m.current_voice_channel_id] as [
                string,
                string | null,
              ],
          ),
        );
        // Mute state seeded only for users actually in voice — others
        // don't have a meaningful state to render and would crowd the
        // store unnecessarily.
        seedVoiceMute(
          members
            .filter((m) => m.current_voice_channel_id !== null)
            .map(
              (m) =>
                [
                  m.user_id,
                  {
                    audioMuted: m.voice_audio_muted,
                    videoMuted: m.voice_video_muted,
                  },
                ] as const,
            ),
        );
      },
    },
  );

  return {
    members: data ?? [],
    loading: isLoading,
    refetch: mutate,
  };
}

/** Connect presence WebSocket: keeps the user online and listens for
 *  server-pushed events (voice occupancy + member roster changes). */
export function usePresence() {
  const { session } = useAuth();
  const wsRef = useRef<WebSocket | null>(null);
  const retryDelayRef = useRef(5_000);
  const MAX_RETRY_DELAY = 120_000;

  useEffect(() => {
    if (!session?.token) return;
    let stopped = false;

    // HUB_API is relative ("/api/hub"); resolve against window.location and
    // swap http(s) → ws(s).
    const u = new URL(
      `${HUB_API}/ws/presence/${session.hubId}`,
      window.location.href,
    );
    u.protocol = u.protocol === "https:" ? "wss:" : "ws:";
    u.searchParams.set("token", session.token);
    const url = u.toString();

    const connect = () => {
      if (stopped) return;
      const ws = new WebSocket(url);
      wsRef.current = ws;

      ws.onopen = () => {
        console.log("[Presence] connected");
        retryDelayRef.current = 5_000;
        // Reconnect after a transient drop: roster could have changed
        // while we were offline. One revalidate covers it.
        globalMutate(membersKey(session.hubId));
      };

      ws.onmessage = (ev) => {
        if (typeof ev.data !== "string") return;
        try {
          const msg = JSON.parse(ev.data) as {
            type?: string;
            user_id?: string;
            channel_id?: string | null;
            kind?: string;
            muted?: boolean;
          };
          switch (msg.type) {
            case "voice_occupancy":
              if (typeof msg.user_id === "string") {
                setVoiceOccupancy(msg.user_id, msg.channel_id ?? null);
                // User left voice → drop their mute entry too. Their next
                // join will start from defaults (both muted).
                if (msg.channel_id == null) {
                  clearVoiceMute(msg.user_id);
                }
              }
              break;
            case "voice_mute":
              if (
                typeof msg.user_id === "string" &&
                (msg.kind === "audio" || msg.kind === "video") &&
                typeof msg.muted === "boolean"
              ) {
                setVoiceMute(msg.user_id, msg.kind, msg.muted);
              }
              break;
            case "member_joined":
              // Someone accepted — they're a member now AND no longer pending.
              globalMutate(membersKey(session.hubId));
              globalMutate(pendingInvitesKey(session.hubId));
              break;
            case "member_left":
            case "member_groups_changed":
              // Roster delta — invalidate the SWR cache, every consumer
              // gets the fresh snapshot via the shared cache key.
              globalMutate(membersKey(session.hubId));
              break;
            // Future event types fall through silently.
          }
        } catch {
          /* non-JSON frame, ignore */
        }
      };

      ws.onclose = () => {
        if (stopped) return;
        const delay = retryDelayRef.current;
        console.log(
          `[Presence] disconnected, reconnecting in ${delay / 1000}s`,
        );
        setTimeout(connect, delay);
        retryDelayRef.current = Math.min(delay * 2, MAX_RETRY_DELAY);
      };

      ws.onerror = () => {
        ws.close();
      };
    };

    connect();

    return () => {
      stopped = true;
      const ws = wsRef.current;
      if (ws) {
        ws.onclose = null;
        ws.close();
        wsRef.current = null;
      }
    };
  }, [session]);
}
