"use client";

import { use, useEffect, useMemo } from "react";
import { VoiceChannelView } from "@/components/hub/voice-channel-view";
import { TextChannelView } from "@/components/hub/text-channel-view";
import { useChannels } from "@/hooks/use-channels";
import { useLastTextChannel } from "@/hooks/use-last-text-channel";
import { useVideoCall } from "@/contexts/video-call-context";

export default function ChannelPage({
  params,
}: {
  params: Promise<{ channelId: string }>;
}) {
  const { channelId } = use(params);
  const { channels, loading } = useChannels();
  const { lastTextChannelId, setLastTextChannelId } = useLastTextChannel();
  const { activeVoiceChannelId, joinVoice } = useVideoCall();

  const channel = channels.find((ch) => ch.id === channelId);
  const type = channel?.type ?? "text";

  // The call itself is a property of the hub session. If we're in one, show it
  // on top regardless of what URL the user picked.
  const activeVoiceChannel = useMemo(() => {
    if (!activeVoiceChannelId) return null;
    return channels.find((ch) => ch.id === activeVoiceChannelId) ?? null;
  }, [channels, activeVoiceChannelId]);

  // Bottom tile is the text channel being viewed:
  //  • viewing a text URL → that one
  //  • viewing a voice URL → whatever text channel the user had last
  //    (so re-opening the voice keeps their chat context visible)
  const bottomTextChannel = useMemo(() => {
    if (type === "text" && channel) return channel;
    if (lastTextChannelId) {
      const hit = channels.find(
        (ch) => ch.id === lastTextChannelId && ch.type === "text",
      );
      if (hit) return hit;
    }
    return channels.find((ch) => ch.type === "text") ?? null;
  }, [type, channel, channels, lastTextChannelId]);

  // Remember the last visited text so the voice-tile view can re-surface it.
  useEffect(() => {
    if (type === "text") setLastTextChannelId(channelId);
  }, [type, channelId, setLastTextChannelId]);

  // Landing on a voice URL auto-connects. Noop if already in this call; if in
  // a different call, `joinVoice` disconnects the old one first.
  useEffect(() => {
    if (type === "voice" || type === "stage") {
      void joinVoice(channelId);
    }
  }, [type, channelId, joinVoice]);

  if (loading) {
    return (
      <div className="flex flex-1 items-center justify-center text-muted-foreground">
        <div className="h-5 w-5 animate-spin rounded-full border-2 border-primary border-t-transparent" />
      </div>
    );
  }

  const inCall = activeVoiceChannel !== null;

  return (
    <div className="flex flex-1 flex-col overflow-hidden">
      {inCall && (
        <div className="flex flex-1 flex-col overflow-hidden border-b">
          <VoiceChannelView
            channelId={activeVoiceChannel.id}
            channelName={activeVoiceChannel.name}
          />
        </div>
      )}
      <div className="flex flex-1 flex-col overflow-hidden">
        {bottomTextChannel ? (
          <TextChannelView
            key={bottomTextChannel.id}
            channelId={bottomTextChannel.id}
            channelName={bottomTextChannel.name}
          />
        ) : (
          <div className="flex flex-1 items-center justify-center text-xs text-muted-foreground">
            Pick a text channel on the left
          </div>
        )}
      </div>
    </div>
  );
}
