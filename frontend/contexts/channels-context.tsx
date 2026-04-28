import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useState,
  type ReactNode,
} from "react";
import { useAuth } from "@/lib/auth";

const HUB_API = "/api/hub";

export type ChannelKind = "text" | "voice" | "stage" | "dm";

export interface Channel {
  id: string;
  hub_id: string;
  name: string;
  type: ChannelKind;
  position: number;
  icon_id: string | null;
  icon_color: string | null;
  icon_image_url: string | null;
  /** Stringified user ids. Only present for type === "dm". */
  participants?: string[];
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

interface ChannelsValue {
  channels: Channel[];
  loading: boolean;
  fetchChannels: () => Promise<void>;
  createChannel: (data: CreateChannelRequest) => Promise<Channel | null>;
  updateChannel: (
    channelId: string,
    data: UpdateChannelRequest,
  ) => Promise<Channel | null>;
  deleteChannel: (channelId: string) => Promise<boolean>;
  uploadIcon: (channelId: string, file: File) => Promise<string | null>;
  /** Get-or-create a 1:1 DM channel with `recipientId`. Idempotent on the
   * server — returning the same channel for repeated calls. */
  openDm: (recipientId: string) => Promise<Channel | null>;
}

const ChannelsContext = createContext<ChannelsValue | null>(null);

export function ChannelsProvider({ children }: { children: ReactNode }) {
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
      const [channelsRes, dmsRes] = await Promise.all([
        fetch(`${HUB_API}/v1/hubs/${session.hubId}/channels`, {
          headers: headers(),
        }),
        fetch(`${HUB_API}/v1/hubs/${session.hubId}/dms`, {
          headers: headers(),
        }),
      ]);

      const regular: Channel[] = channelsRes.ok ? await channelsRes.json() : [];
      const dms: Channel[] = dmsRes.ok ? await dmsRes.json() : [];

      // /channels excludes type='dm' on the server side; merge here so the
      // sidebar consumes one ordered array.
      setChannels([...regular, ...dms]);
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
    async (
      channelId: string,
      data: UpdateChannelRequest,
    ): Promise<Channel | null> => {
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
      setChannels((prev) =>
        prev.map((ch) => (ch.id === channelId ? updated : ch)),
      );
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

  const openDm = useCallback(
    async (recipientId: string): Promise<Channel | null> => {
      if (!session?.hubId) return null;
      const res = await fetch(`${HUB_API}/v1/hubs/${session.hubId}/dms`, {
        method: "POST",
        headers: headers(),
        body: JSON.stringify({ recipient_id: recipientId }),
      });
      if (!res.ok) return null;
      const channel: Channel = await res.json();
      // Upsert: idempotent on the server, so the channel may already be in
      // state from a previous open or from fetchChannels — replace by id.
      setChannels((prev) => {
        const without = prev.filter((c) => c.id !== channel.id);
        return [...without, channel];
      });
      return channel;
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
      setChannels((prev) =>
        prev.map((ch) =>
          ch.id === channelId ? { ...ch, icon_image_url: data.url } : ch,
        ),
      );
      return data.url as string;
    },
    [session?.hubId, session?.token],
  );

  return (
    <ChannelsContext.Provider
      value={{
        channels,
        loading,
        fetchChannels,
        createChannel,
        updateChannel,
        deleteChannel,
        uploadIcon,
        openDm,
      }}
    >
      {children}
    </ChannelsContext.Provider>
  );
}

export function useChannels(): ChannelsValue {
  const ctx = useContext(ChannelsContext);
  if (!ctx) {
    throw new Error("useChannels must be used within <ChannelsProvider>");
  }
  return ctx;
}
