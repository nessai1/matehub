// Friendly end-of-life screen for temp guests. AuthProvider flips
// `sessionExpired` true either via a client-side timer at JWT exp or via a
// 401 from any API call, whichever fires first. We respond by:
//   1. yanking the user out of any voice call (their JWT is no good
//      anyway and the SFU will drop them shortly — better to do it cleanly
//      from our side than wait for the disconnect)
//   2. showing a non-dismissable modal with the message and an "OK" button
//   3. on OK, AuthProvider clears the session and routes to /login
//
// Lives inside HubLayout, below VideoCallProvider, so leaveVoice works.
//
// Built on plain Dialog (rather than shadcn's AlertDialog) because the
// radix-ui meta package's AlertDialog types don't surface className for
// our compiler config — and we don't need AlertDialog semantics here, we
// just disable the dismiss paths manually.

import { useEffect } from "react";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { useAuth } from "@/lib/auth";
import { useVideoCall } from "@/contexts/video-call-context";
import { t } from "@/i18n";

export function SessionExpiredDialog() {
  const { sessionExpired, acknowledgeSessionExpired } = useAuth();
  const { leaveVoice } = useVideoCall();

  useEffect(() => {
    if (sessionExpired) leaveVoice();
  }, [sessionExpired, leaveVoice]);

  return (
    <Dialog
      open={sessionExpired}
      // Ignore overlay clicks / explicit close requests — the only way out
      // is the OK button, which acknowledges and routes to /login.
      onOpenChange={() => {}}
    >
      <DialogContent
        // Block Esc and outside-click dismissal. Same intent as the
        // onOpenChange noop above; both belt-and-suspenders since the user
        // is being told "your session is over", not asked permission.
        onEscapeKeyDown={(e: KeyboardEvent) => e.preventDefault()}
        onPointerDownOutside={(e: Event) => e.preventDefault()}
        onInteractOutside={(e: Event) => e.preventDefault()}
        showCloseButton={false}
      >
        <DialogHeader>
          <DialogTitle>{t("Your session has ended")}</DialogTitle>
          <DialogDescription>
            {t(
              "Your guest access has expired. Contact the person who invited you to get a new link.",
            )}
          </DialogDescription>
        </DialogHeader>
        <DialogFooter>
          <Button onClick={acknowledgeSessionExpired}>{t("OK")}</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
