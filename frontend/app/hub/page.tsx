import { useEffect, useMemo } from "react";
import { VideoWorkspace } from "@/components/hub/video-workspace/video-workspace";
import {
  ChatWorkspace,
  CollapsedChatBar,
} from "@/components/hub/chat-workspace";
import { WorkspaceStack } from "@/components/hub/workspace-stack";
import { useChannels } from "@/hooks/use-channels";
import { useVideoCall } from "@/contexts/video-call-context";
import { useHubSelection } from "@/contexts/hub-selection-context";
import type { Channel } from "@/components/nav-channels";
import type { Channel as ApiChannel } from "@/hooks/use-channels";

function toChannel(ch: ApiChannel): Channel {
  return {
    id: ch.id,
    name: ch.name,
    type: ch.type,
    position: ch.position,
    iconId: ch.icon_id ?? undefined,
    iconColor: ch.icon_color,
    iconImage: ch.icon_image_url,
    participants: ch.participants,
  };
}

/**
 * The whole hub is a single SPA screen. Text-channel selection and voice-call
 * state are context, not URL — clicking around the sidebar never navigates.
 */
export default function HubPage() {
  const { channels, loading } = useChannels();
  const { activeVoiceChannelId } = useVideoCall();
  const { selectedTextChannelId, selectTextChannel } = useHubSelection();

  // Resolve IDs into channels.
  const voiceChannel = useMemo(() => {
    if (!activeVoiceChannelId) return null;
    return channels.find((ch) => ch.id === activeVoiceChannelId) ?? null;
  }, [channels, activeVoiceChannelId]);

  const textChannel = useMemo(() => {
    // Selection slot is shared between text channels and DMs — both render
    // through ChatWorkspace, just with different headers.
    if (selectedTextChannelId) {
      const hit = channels.find(
        (ch) =>
          ch.id === selectedTextChannelId &&
          (ch.type === "text" || ch.type === "dm"),
      );
      if (hit) return hit;
    }
    // No selection yet → fall back to the first text channel so the chat slot
    // isn't empty on first login.
    return channels.find((ch) => ch.type === "text") ?? null;
  }, [channels, selectedTextChannelId]);

  // On first load, pin the fallback into the selection so subsequent UI uses
  // the same value as the sidebar's "active" highlight.
  useEffect(() => {
    if (!selectedTextChannelId && textChannel) {
      selectTextChannel(textChannel.id);
    }
  }, [selectedTextChannelId, textChannel, selectTextChannel]);

  if (loading) {
    return (
      <div className="flex flex-1 items-center justify-center text-muted-foreground">
        <div className="h-5 w-5 animate-spin rounded-full border-2 border-primary border-t-transparent" />
      </div>
    );
  }

  const inCall = voiceChannel !== null;

  const top = inCall ? (
    <VideoWorkspace channel={toChannel(voiceChannel)} />
  ) : null;

  const bottom = textChannel ? (
    <ChatWorkspace
      channel={toChannel(textChannel)}
      standalone={!inCall}
    />
  ) : null;

  const collapsedBottom = textChannel ? (
    <CollapsedChatBar channel={toChannel(textChannel)} />
  ) : undefined;

  return (
    <WorkspaceStack top={top} bottom={bottom} collapsedBottom={collapsedBottom} />
  );
}
