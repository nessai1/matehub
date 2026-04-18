"use client";

import { useCallback, useEffect, useState } from "react";
import { useAuth } from "@/lib/auth";

const HUB_API = process.env.NEXT_PUBLIC_HUB_API_URL || "http://localhost:3002";

export interface Channel {
  id: string;
  hub_id: string;
  name: string;
  type: "text" | "voice" | "stage";
  position: number;
  icon_id: string | null;
  icon_color: string | null;
  icon_image_url: string | null;
  created_at: string;
}

export interface CreateChannelRequest {
  name: string;
  type: string;
  icon_id?: string | null;
  icon_color?: string | null;
}

export interface UpdateChannelRequest {
  name?: string;
  position?: number;
  icon_id?: string | null;
  icon_color?: string | null;
  icon_image_url?: string | null;
}

export function useChannels() {
  const { session } = useAuth();
  const [channels, setChannels] = useState<Channel[]>([]);
  const [loading, setLoading] = useState(true);

  const headers = useCallback((): Record<string, string> => {
    if (!session?.token) return {};
    return {
      "Content-Type": "application/json",
      Authorization: `Bearer ${session.token}`,
    };
  }, [session?.token]);

  const fetchChannels = useCallback(async () => {
    if (!session?.hubId || !session?.token) return;
    try {
      const res = await fetch(
        `${HUB_API}/v1/hubs/${session.hubId}/channels`,
        { headers: headers() },
      );
      if (res.ok) {
        setChannels(await res.json());
      }
    } catch {
      // ignore
    } finally {
      setLoading(false);
    }
  }, [session?.hubId, session?.token, headers]);

  useEffect(() => {
    fetchChannels();
  }, [fetchChannels]);

  const createChannel = useCallback(
    async (data: CreateChannelRequest): Promise<Channel | null> => {
      if (!session?.hubId) return null;
      const res = await fetch(
        `${HUB_API}/v1/hubs/${session.hubId}/channels`,
        {
          method: "POST",
          headers: headers(),
          body: JSON.stringify(data),
        },
      );
      if (!res.ok) return null;
      const channel: Channel = await res.json();
      setChannels((prev) => [...prev, channel]);
      return channel;
    },
    [session?.hubId, headers],
  );

  const updateChannel = useCallback(
    async (channelId: string, data: UpdateChannelRequest): Promise<Channel | null> => {
      if (!session?.hubId) return null;
      const res = await fetch(
        `${HUB_API}/v1/hubs/${session.hubId}/channels/${channelId}`,
        {
          method: "PATCH",
          headers: headers(),
          body: JSON.stringify(data),
        },
      );
      if (!res.ok) return null;
      const updated: Channel = await res.json();
      setChannels((prev) => prev.map((ch) => (ch.id === channelId ? updated : ch)));
      return updated;
    },
    [session?.hubId, headers],
  );

  const deleteChannel = useCallback(
    async (channelId: string): Promise<boolean> => {
      if (!session?.hubId) return false;
      const res = await fetch(
        `${HUB_API}/v1/hubs/${session.hubId}/channels/${channelId}`,
        {
          method: "DELETE",
          headers: headers(),
        },
      );
      if (res.ok) {
        setChannels((prev) => prev.filter((ch) => ch.id !== channelId));
        return true;
      }
      return false;
    },
    [session?.hubId, headers],
  );

  const uploadIcon = useCallback(
    async (channelId: string, file: File): Promise<string | null> => {
      if (!session?.hubId || !session?.token) return null;
      const formData = new FormData();
      formData.append("icon", file);
      const res = await fetch(
        `${HUB_API}/v1/hubs/${session.hubId}/channels/${channelId}/icon`,
        {
          method: "POST",
          headers: { Authorization: `Bearer ${session.token}` },
          body: formData,
        },
      );
      if (!res.ok) return null;
      const data = await res.json();
      // Update local state with new icon URL
      setChannels((prev) =>
        prev.map((ch) =>
          ch.id === channelId ? { ...ch, icon_image_url: data.url } : ch,
        ),
      );
      return data.url as string;
    },
    [session?.hubId, session?.token],
  );

  return { channels, loading, fetchChannels, createChannel, updateChannel, deleteChannel, uploadIcon };
}
