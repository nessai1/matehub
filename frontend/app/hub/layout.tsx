import { Suspense } from "react";
import { Outlet } from "react-router";
import { AppSidebar } from "@/components/app-sidebar";
import { AuthGuard } from "@/components/auth-guard";
import { MemberSidebar } from "@/components/hub/member-sidebar";
import { SidebarInset, SidebarProvider } from "@/components/ui/sidebar";
import { TooltipProvider } from "@/components/ui/tooltip";
import { VideoCallProvider } from "@/contexts/video-call-context";
import { HubSelectionProvider } from "@/contexts/hub-selection-context";
import { ChatProvider } from "@/contexts/chat-context";

export function HubLayout() {
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
              {/* ChatProvider holds the single chat WS for the whole hub
                  session. Moving it above SidebarProvider means opening the
                  channel sidebar never unmounts the client. */}
              <ChatProvider>
                <SidebarProvider>
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
              </ChatProvider>
            </HubSelectionProvider>
          </VideoCallProvider>
        </TooltipProvider>
      </AuthGuard>
    </Suspense>
  );
}

export default HubLayout;
