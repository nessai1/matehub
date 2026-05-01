import { HubSwitcher } from "@/components/hub-switcher"
import { NavChannels } from "@/components/nav-channels"
import { NavUser } from "@/components/nav-user"
import { useAuth } from "@/lib/auth"
import { useHub } from "@/hooks/use-hub"
import {
  Sidebar,
  SidebarContent,
  SidebarFooter,
  SidebarHeader,
} from "@/components/ui/sidebar"
import { Suspense } from "react"

export function AppSidebar(props: React.ComponentProps<typeof Sidebar>) {
  const { session } = useAuth()
  const { hub } = useHub()

  if (!session) return null

  return (
    <Sidebar collapsible="offcanvas" {...props}>
      <SidebarHeader>
        <HubSwitcher
          currentHub={{
            id: session.hubId,
            name: hub?.name ?? session.hubSlug,
            slug: hub?.slug ?? session.hubSlug,
            plan: hub?.plan ?? "free",
            avatarUrl: hub?.avatar_url ?? null,
            description: hub?.description ?? null,
          }}
          otherHubs={[]}
        />
      </SidebarHeader>
      <SidebarContent>
        <Suspense>
          <NavChannels />
        </Suspense>
      </SidebarContent>
      <SidebarFooter>
        <NavUser
          user={{
            name: session.displayName,
            username: session.username,
          }}
        />
      </SidebarFooter>
    </Sidebar>
  )
}
