// Channel state lives in a context provider so all consumers share the same
// list. See contexts/channels-context.tsx for the implementation.
export {
  useChannels,
  type Channel,
  type CreateChannelRequest,
  type UpdateChannelRequest,
} from "@/contexts/channels-context";
