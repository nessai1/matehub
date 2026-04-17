import { Suspense } from "react";
import { AppSidebar } from "@/components/app-sidebar";
import { AuthGuard } from "@/components/auth-guard";
import { MemberSidebar } from "@/components/hub/member-sidebar";
import { SidebarInset, SidebarProvider } from "@/components/ui/sidebar";
import { TooltipProvider } from "@/components/ui/tooltip";

export default function HubLayout({
  children,
}: {
  children: React.ReactNode;
}) {
  return (
    <Suspense>
      <AuthGuard>
        <TooltipProvider>
          <SidebarProvider>
            <AppSidebar />
            <SidebarInset>
              <div className="flex h-screen flex-col overflow-hidden">
                <div className="flex flex-1 overflow-hidden">
                  <main className="flex flex-1 flex-col overflow-hidden">
                    {children}
                  </main>
                  <Suspense>
                    <MemberSidebar />
                  </Suspense>
                </div>
              </div>
            </SidebarInset>
          </SidebarProvider>
        </TooltipProvider>
      </AuthGuard>
    </Suspense>
  );
}
