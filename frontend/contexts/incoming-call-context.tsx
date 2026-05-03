import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useState,
  type ReactNode,
} from "react";
import { useAuth } from "@/lib/auth";
import { useChannels } from "@/hooks/use-channels";
import { useChatContext } from "@/contexts/chat-context";
import { useVideoCall } from "@/contexts/video-call-context";

const CHAT_API = "/api/chat";

export interface IncomingCall {
  fromUserId: string;
  channelId: string;
}

/**
 * Active DM call from this client's POV. Lives across the connect/disconnect
 * dance so the watcher effect knows what to send when the user leaves.
 *
 * `startedAt` is the millisecond timestamp at which the call became live —
 * for the recipient that's `accept` time, for the inviter it's the moment
 * the peer first appears in `participants`. While `startedAt === null` the
 * call is still in the ringing phase: leaving means cancel, not end.
 */
interface DmCall {
  channelId: string;
  role: "inviter" | "recipient";
  startedAt: number | null;
}

export type DeclineReason = "declined" | "timeout";

interface IncomingCallValue {
  /** Active invite the user hasn't yet accepted/declined. */
  incomingCall: IncomingCall | null;
  /** Channel id of the DM call we initiated and are still in the ringing
   * phase for; null otherwise. The ringing-tile placeholder uses this. */
  outgoingCallChannelId: string | null;
  /** Inviter side: notify the peer + join the SFU session. */
  startCall: (channelId: string) => Promise<void>;
  /** Recipient side: join the SFU session and clear the dialog. */
  accept: () => Promise<void>;
  /** Recipient side: tell the inviter, then clear the dialog. `reason`
   * decides the system-message text on the backend. */
  decline: (reason?: DeclineReason) => Promise<void>;
}

const IncomingCallContext = createContext<IncomingCallValue | null>(null);

export function useIncomingCall(): IncomingCallValue {
  const ctx = useContext(IncomingCallContext);
  if (!ctx) {
    throw new Error(
      "useIncomingCall must be used within <IncomingCallProvider>",
    );
  }
  return ctx;
}

export function IncomingCallProvider({ children }: { children: ReactNode }) {
  const { session } = useAuth();
  const { client } = useChatContext();
  const { channels } = useChannels();
  const { joinVoice, leaveVoice, activeVoiceChannelId, participants } =
    useVideoCall();
  const [incomingCall, setIncomingCall] = useState<IncomingCall | null>(null);
  const [dmCall, setDmCall] = useState<DmCall | null>(null);

  const auth = useCallback(
    () =>
      session?.token
        ? { Authorization: `Bearer ${session.token}` }
        : ({} as Record<string, string>),
    [session?.token],
  );

  const post = useCallback(
    (path: string, body: unknown) =>
      fetch(`${CHAT_API}${path}`, {
        method: "POST",
        headers: { "Content-Type": "application/json", ...auth() },
        body: JSON.stringify(body),
      }).catch((e) => console.warn("[dm-call] POST failed", path, e)),
    [auth],
  );

  // ── Listen for invite/decline/cancel/ended events ────────────────────────

  useEffect(() => {
    if (!client) return;
    return client.on((event) => {
      switch (event.type) {
        case "dm.call.invite": {
          // Echo of our own /call/start lands here too — ignore it.
          if (event.data.from_user_id === session?.userId) return;
          setIncomingCall({
            fromUserId: event.data.from_user_id,
            channelId: event.data.channel_id,
          });
          break;
        }
        case "dm.call.decline": {
          if (event.data.from_user_id === session?.userId) return;
          // Inviter side: peer rejected before pickup. Drop the call out from
          // under us. Backend wrote the system message based on `reason`.
          setDmCall((prev) => {
            if (prev && prev.channelId === event.data.channel_id) {
              // Clear state first so the leave-watcher doesn't double-POST.
              return null;
            }
            return prev;
          });
          if (activeVoiceChannelId === event.data.channel_id) {
            leaveVoice();
          }
          break;
        }
        case "dm.call.cancel": {
          // Recipient side: inviter gave up while we were still ringing.
          setIncomingCall((prev) =>
            prev && prev.channelId === event.data.channel_id ? null : prev,
          );
          break;
        }
        case "dm.call.ended": {
          if (event.data.from_user_id === session?.userId) return;
          // Peer hung up after pickup. Clear state so our own leaveVoice
          // doesn't fire a second `/call/end` and double the system message.
          setDmCall((prev) =>
            prev && prev.channelId === event.data.channel_id ? null : prev,
          );
          if (activeVoiceChannelId === event.data.channel_id) {
            leaveVoice();
          }
          break;
        }
      }
    });
  }, [client, session?.userId, activeVoiceChannelId, leaveVoice]);

  // ── Inviter: stamp startedAt the first time the peer's participant lands ──

  useEffect(() => {
    if (!dmCall || dmCall.role !== "inviter" || dmCall.startedAt !== null) {
      return;
    }
    const ch = channels.find((c) => c.id === dmCall.channelId);
    const peerId = ch?.participants?.find((id) => id !== session?.userId);
    if (!peerId) return;
    const peerJoined = participants.some((p) => p.userId === peerId);
    if (peerJoined) {
      const stamp = Date.now();
      // Microtask-dispatch the setState so the effect body itself stays
      // side-effect-only; React still batches into the next render.
      queueMicrotask(() => {
        setDmCall((prev) =>
          prev && prev.startedAt === null ? { ...prev, startedAt: stamp } : prev,
        );
      });
    }
  }, [dmCall, participants, channels, session?.userId]);

  // ── Watcher: when we leave the SFU, decide cancel vs end ─────────────────
  //
  // Runs on every activeVoiceChannelId change. If we have a dmCall set and
  // we just dropped out of its channel — fire the appropriate REST call and
  // cleanup. Idempotency: an inbound dm.call.ended event clears `dmCall`
  // before triggering leaveVoice, so this branch sees a null state and
  // doesn't re-POST.

  useEffect(() => {
    if (!dmCall) return;
    if (activeVoiceChannelId === dmCall.channelId) return;
    // Dropped. Capture before clearing. State update goes through a
    // microtask so the effect body stays side-effect-only.
    const left = dmCall;
    queueMicrotask(() => setDmCall(null));
    if (left.startedAt != null) {
      const duration = Math.max(
        0,
        Math.floor((Date.now() - left.startedAt) / 1000),
      );
      void post(`/v1/dms/${left.channelId}/call/end`, {
        duration_secs: duration,
      });
    } else {
      void post(`/v1/dms/${left.channelId}/call/cancel`, {});
    }
  }, [activeVoiceChannelId, dmCall, post]);

  // ── Public API ────────────────────────────────────────────────────────────

  const startCall = useCallback(
    async (channelId: string) => {
      setDmCall({ channelId, role: "inviter", startedAt: null });
      await Promise.all([
        post(`/v1/dms/${channelId}/call/start`, {}),
        joinVoice(channelId),
      ]);
    },
    [joinVoice, post],
  );

  const accept = useCallback(async () => {
    if (!incomingCall) return;
    const channelId = incomingCall.channelId;
    setIncomingCall(null);
    setDmCall({ channelId, role: "recipient", startedAt: Date.now() });
    await joinVoice(channelId);
  }, [incomingCall, joinVoice]);

  const decline = useCallback(
    async (reason: DeclineReason = "declined") => {
      if (!incomingCall) return;
      const channelId = incomingCall.channelId;
      setIncomingCall(null);
      await post(`/v1/dms/${channelId}/call/decline`, { reason });
    },
    [incomingCall, post],
  );

  const outgoingCallChannelId =
    dmCall?.role === "inviter" && dmCall.startedAt === null
      ? dmCall.channelId
      : null;

  return (
    <IncomingCallContext.Provider
      value={{
        incomingCall,
        outgoingCallChannelId,
        startCall,
        accept,
        decline,
      }}
    >
      {children}
    </IncomingCallContext.Provider>
  );
}
