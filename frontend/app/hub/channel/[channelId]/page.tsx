"use client";

import { use } from "react";
import { VoiceChannelView } from "@/components/hub/voice-channel-view";
import { TextChannelView } from "@/components/hub/text-channel-view";
import { useChannels } from "@/hooks/use-channels";

export default function ChannelPage({
  params,
}: {
  params: Promise<{ channelId: string }>;
}) {
  const { channelId } = use(params);
  const { channels, loading } = useChannels();

  const channel = channels.find((ch) => ch.id === channelId);
  const name = channel?.name ?? channelId;
  const type = channel?.type ?? "text";

  if (loading) {
    return (
      <div className="flex flex-1 items-center justify-center text-muted-foreground">
        <div className="h-5 w-5 animate-spin rounded-full border-2 border-primary border-t-transparent" />
      </div>
    );
  }

  if (type === "voice" || type === "stage") {
    return <VoiceChannelView channelId={channelId} channelName={name} />;
  }

  return <TextChannelView channelId={channelId} channelName={name} />;
}
