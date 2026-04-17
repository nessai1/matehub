"use client";

import { use } from "react";
import { Hash, Mic, Radio } from "lucide-react";
import { VoiceChannelView } from "@/components/hub/voice-channel-view";
import { TextChannelView } from "@/components/hub/text-channel-view";

type ChannelType = "text" | "voice" | "stage";

// Hard-coded channel data -- will come from API
const channelData: Record<string, { name: string; type: ChannelType }> = {
  "8001": { name: "general", type: "text" },
  "8002": { name: "random", type: "text" },
  "8003": { name: "voice-test", type: "voice" },
  "8004": { name: "stage-test", type: "stage" },
};

const icons: Record<ChannelType, typeof Hash> = {
  text: Hash,
  voice: Mic,
  stage: Radio,
};

export default function ChannelPage({
  params,
}: {
  params: Promise<{ channelId: string }>;
}) {
  const { channelId } = use(params);
  const channel = channelData[channelId] ?? {
    name: channelId,
    type: "text" as ChannelType,
  };
  const Icon = icons[channel.type];

  const isVoice = channel.type === "voice" || channel.type === "stage";

  if (isVoice) {
    return <VoiceChannelView channelId={channelId} channelName={channel.name} />;
  }

  return <TextChannelView channelId={channelId} channelName={channel.name} />;
}
