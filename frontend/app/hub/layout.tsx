import { Suspense, useEffect, useState } from "react";
import { Outlet } from "react-router";
import { AppSidebar } from "@/components/app-sidebar";
import { AuthGuard } from "@/components/auth-guard";
import { MemberSidebar } from "@/components/hub/member-sidebar";
import { SidebarInset, SidebarProvider } from "@/components/ui/sidebar";
import { TooltipProvider } from "@/components/ui/tooltip";
import { VideoCallProvider } from "@/contexts/video-call-context";
import { HubSelectionProvider } from "@/contexts/hub-selection-context";
import { ChannelsProvider } from "@/contexts/channels-context";
import { ChatProvider } from "@/contexts/chat-context";
import { IncomingCallProvider } from "@/contexts/incoming-call-context";
import { AddTeammatesProvider } from "@/contexts/add-teammates-context";
import { MemberSidebarProvider } from "@/contexts/member-sidebar-context";
import { IncomingCallDialog } from "@/components/hub/incoming-call-dialog";

// Mirror member-sidebar-context's COLLAPSE_BREAKPOINT so both sidebars react
// to the same threshold. Media query wins on every resize event — manual
// toggles work for the current viewport, then a breakpoint crossing resets.
const LEFT_COLLAPSE_BREAKPOINT = 1024;

function useLeftSidebarOpen() {
  const [open, setOpen] = useState<boolean>(() => {
    if (typeof window !== "undefined") {
      return window.innerWidth >= LEFT_COLLAPSE_BREAKPOINT;
    }
    return true;
  });

  useEffect(() => {
    if (typeof window === "undefined") return;
    const mql = window.matchMedia(`(min-width: ${LEFT_COLLAPSE_BREAKPOINT}px)`);
    const apply = () => setOpen(mql.matches);
    apply();
    mql.addEventListener("change", apply);
    return () => mql.removeEventListener("change", apply);
  }, []);

  return [open, setOpen] as const;
}

export function HubLayout() {
  const [leftOpen, setLeftOpen] = useLeftSidebarOpen();
  return (
    <Suspense>
      <AuthGuard>
        <TooltipProvider>
          {/* VideoCallProvider + HubSelectionProvider live above the sidebar
              and main content, so moving between channels in the UI never
              unmounts the call or the chat selection — both are hub-session
              state, not page-level state. */}
          <VideoCallProvider>
            <HubSelectionProvider>
              {/* ChannelsProvider gives the sidebar and the workspace a single
                  view of the channel list — without it, each useChannels()
                  call kept its own state and creates/updates didn't propagate. */}
              <ChannelsProvider>
                {/* ChatProvider holds the single chat WS for the whole hub
                    session. Moving it above SidebarProvider means opening the
                    channel sidebar never unmounts the client. */}
                <ChatProvider>
                  {/* IncomingCallProvider sits below ChatProvider — it
                      subscribes to chat-WS events for DM ringing and lives
                      hub-wide so the modal pops up regardless of route. */}
                  <IncomingCallProvider>
                    <AddTeammatesProvider>
                      <MemberSidebarProvider>
                        <SidebarProvider open={leftOpen} onOpenChange={setLeftOpen}>
                          <AppSidebar />
                          <SidebarInset>
                            <div className="flex h-screen flex-col overflow-hidden p-2 pl-0">
                              <div className="flex flex-1 gap-2 overflow-hidden">
                                <main className="flex flex-1 flex-col overflow-hidden rounded-2xl bg-background">
                                  <Outlet />
                                </main>
                                <Suspense>
                                  <MemberSidebar />
                                </Suspense>
                              </div>
                            </div>
                          </SidebarInset>
                        </SidebarProvider>
                        <IncomingCallDialog />
                      </MemberSidebarProvider>
                    </AddTeammatesProvider>
                  </IncomingCallProvider>
                </ChatProvider>
              </ChannelsProvider>
            </HubSelectionProvider>
          </VideoCallProvider>
        </TooltipProvider>
      </AuthGuard>
    </Suspense>
  );
}

export default HubLayout;
