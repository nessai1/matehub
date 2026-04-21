import { useCallback, useEffect, useRef, useState } from "react";
import { useAuth } from "@/lib/auth";
import {
  seedVoiceOccupancy,
  setVoiceOccupancy,
} from "@/lib/voice-occupancy-store";

const HUB_API = import.meta.env.VITE_HUB_API_URL || "http://localhost:3002";

export interface MemberGroup {
  id: number;
  name: string;
  color: string | null;
}

export interface Member {
  user_id: number;
  username: string;
  display_name: string;
  avatar_url: string | null;
  is_online: boolean;
  last_seen_at: string | null;
  groups: MemberGroup[];
  user_type: "permanent" | "temp";
  expires_at: string | null;
  /** Voice channel the member is currently in (null = not in voice). */
  current_voice_channel_id: number | null;
}

export function useMembers() {
  const { session } = useAuth();
  const [members, setMembers] = useState<Member[]>([]);
  const [loading, setLoading] = useState(true);

  const fetchMembers = useCallback(async () => {
    if (!session?.token) return;
    try {
      const res = await fetch(
        `${HUB_API}/v1/hubs/${session.hubId}/members-full`,
        {
          headers: { Authorization: `Bearer ${session.token}` },
        },
      );
      if (res.status === 401) {
        // Token expired -- force re-login
        window.location.href = "/login";
        return;
      }
      if (res.ok) {
        const fetched: Member[] = await res.json();
        setMembers(fetched);
        // Seed the occupancy store from the authoritative snapshot. Live WS
        // events take over from here.
        seedVoiceOccupancy(
          fetched.map(
            (m) => [m.user_id, m.current_voice_channel_id] as [number, number | null],
          ),
        );
      }
    } catch {
      // silent fail -- will retry on next poll
    } finally {
      setLoading(false);
    }
  }, [session]);

  // Fetch immediately (show member list fast), then refetch after 2s
  // (by then presence WS is connected and Redis has online status)
  useEffect(() => {
    fetchMembers();
    const presenceDelay = setTimeout(fetchMembers, 2000);
    const iv = setInterval(fetchMembers, 10_000);
    return () => {
      clearTimeout(presenceDelay);
      clearInterval(iv);
    };
  }, [fetchMembers]);

  return { members, loading, refetch: fetchMembers };
}

/** Connect presence WebSocket to keep current user online */
export function usePresence() {
  const { session } = useAuth();
  const wsRef = useRef<WebSocket | null>(null);
  const retryDelayRef = useRef(5_000);
  const MAX_RETRY_DELAY = 120_000;

  useEffect(() => {
    if (!session?.token) return;
    let stopped = false;

    const wsProto = HUB_API.startsWith("https") ? "wss" : "ws";
    const host = HUB_API.replace(/^https?:\/\//, "");
    const url = `${wsProto}://${host}/ws/presence/${session.hubId}?token=${session.token}`;

    const connect = () => {
      if (stopped) return;
      const ws = new WebSocket(url);
      wsRef.current = ws;

      ws.onopen = () => {
        console.log("[Presence] connected");
        retryDelayRef.current = 5_000;
      };

      ws.onmessage = (ev) => {
        // Hub pushes server-initiated events as JSON frames (currently just
        // voice_occupancy). Text frames we don't recognise are ignored —
        // future event types land here too.
        if (typeof ev.data !== "string") return;
        try {
          const msg = JSON.parse(ev.data) as {
            type?: string;
            user_id?: number;
            channel_id?: number | null;
          };
          if (
            msg.type === "voice_occupancy" &&
            typeof msg.user_id === "number"
          ) {
            setVoiceOccupancy(msg.user_id, msg.channel_id ?? null);
          }
        } catch {
          /* non-JSON frame, ignore */
        }
      };

      ws.onclose = () => {
        if (stopped) return;
        const delay = retryDelayRef.current;
        console.log(`[Presence] disconnected, reconnecting in ${delay / 1000}s`);
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
