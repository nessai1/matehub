import { useEffect } from "react";
import { PhoneIcon, PhoneOffIcon } from "lucide-react";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Avatar, AvatarFallback, AvatarImage } from "@/components/ui/avatar";
import { Button } from "@/components/ui/button";
import { useIncomingCall } from "@/contexts/incoming-call-context";
import { useMembers } from "@/hooks/use-members";
import { startCallSoundLoop } from "@/lib/call-sounds";

/**
 * Persistent incoming-call modal. Mounted at hub level so it pops up no matter
 * which channel the recipient is currently looking at. Auto-declines after
 * RING_TIMEOUT_MS so a forgotten browser tab doesn't ring forever.
 */
const RING_TIMEOUT_MS = 30_000;

export function IncomingCallDialog() {
  const { incomingCall, accept, decline } = useIncomingCall();
  const { members } = useMembers();

  const peer = incomingCall
    ? members.find((m) => m.user_id === incomingCall.fromUserId)
    : null;
  const displayName = peer?.display_name ?? incomingCall?.fromUserId ?? "Unknown";

  // Continuous ringtone while the dialog is open. The audio element has
  // `loop = true` set inside startCallSoundLoop, so it seamlessly repeats
  // until we call the returned stop fn on unmount or when the call clears.
  useEffect(() => {
    if (!incomingCall) return;
    return startCallSoundLoop("incoming_call");
  }, [incomingCall]);

  // 30 s timeout → silently auto-decline. Pass reason="timeout" so the
  // backend writes "X didn't answer" rather than "X declined".
  useEffect(() => {
    if (!incomingCall) return;
    const t = setTimeout(() => void decline("timeout"), RING_TIMEOUT_MS);
    return () => clearTimeout(t);
  }, [incomingCall, decline]);

  return (
    <Dialog open={!!incomingCall} onOpenChange={(o: boolean) => !o && void decline()}>
      <DialogContent className="sm:max-w-[280px]">
        <DialogHeader className="space-y-0.5">
          <DialogTitle className="text-center text-base">Incoming call</DialogTitle>
          <DialogDescription className="text-center text-xs">
            {displayName} is calling
          </DialogDescription>
        </DialogHeader>

        <div className="flex flex-col items-center gap-3 py-2">
          <Avatar className="h-16 w-16">
            {peer?.avatar_url && <AvatarImage src={peer.avatar_url} />}
            <AvatarFallback className="bg-muted text-lg font-bold text-muted-foreground">
              {displayName.charAt(0).toUpperCase()}
            </AvatarFallback>
          </Avatar>

          <div className="flex items-center gap-3">
            <Button
              variant="outline"
              className="h-10 w-10 rounded-full border-red-200 p-0 text-red-500 hover:bg-red-50 dark:border-red-900 dark:text-red-400 dark:hover:bg-red-950"
              onClick={() => void decline()}
              aria-label="Decline call"
            >
              <PhoneOffIcon className="h-4 w-4" />
            </Button>
            <Button
              className="h-10 w-10 rounded-full bg-emerald-600 p-0 text-white hover:bg-emerald-500"
              onClick={() => void accept()}
              aria-label="Accept call"
            >
              <PhoneIcon className="h-4 w-4" />
            </Button>
          </div>
        </div>
      </DialogContent>
    </Dialog>
  );
}
