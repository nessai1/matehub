"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import { useAuth } from "@/lib/auth";

const HUB_API = process.env.NEXT_PUBLIC_HUB_API_URL || "http://localhost:3002";

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
  expires_at: string | null;
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
        setMembers(await res.json());
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
