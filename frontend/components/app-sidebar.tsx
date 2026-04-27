import { UserPlusIcon } from "lucide-react"
import { HubSwitcher } from "@/components/hub-switcher"
import { NavChannels } from "@/components/nav-channels"
import { NavUser } from "@/components/nav-user"
import { useAuth } from "@/lib/auth"
import { usePermissions, P } from "@/hooks/use-permissions"
import { useAddTeammates } from "@/contexts/add-teammates-context"
import {
  Sidebar,
  SidebarContent,
  SidebarFooter,
  SidebarGroup,
  SidebarHeader,
  SidebarMenu,
  SidebarMenuButton,
  SidebarMenuItem,
} from "@/components/ui/sidebar"
import { Suspense } from "react"

export function AppSidebar(props: React.ComponentProps<typeof Sidebar>) {
  const { session } = useAuth()
  const { has } = usePermissions()
  const { open: openAddTeammates } = useAddTeammates()
  const canInvite = has(P.INVITE_PERMANENT) || has(P.CREATE_TEMP_LINKS)

  if (!session) return null

  return (
    <Sidebar collapsible="icon" {...props}>
      <SidebarHeader>
        <HubSwitcher
          currentHub={{
            id: session.hubId,
            name: "Dev Hub", // TODO: fetch from API
            slug: session.hubSlug,
            plan: "Pro",
          }}
          otherHubs={[]}
        />
      </SidebarHeader>
      <SidebarContent>
        <Suspense>
          <NavChannels />
        </Suspense>
        {canInvite && (
          <SidebarGroup>
            <SidebarMenu>
              <SidebarMenuItem>
                <SidebarMenuButton onClick={openAddTeammates} tooltip="Add Teammates">
                  <UserPlusIcon />
                  <span>Add Teammates</span>
                </SidebarMenuButton>
              </SidebarMenuItem>
            </SidebarMenu>
          </SidebarGroup>
        )}
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
