"use client"

import { HubSwitcher } from "@/components/hub-switcher"
import { NavChannels } from "@/components/nav-channels"
import { NavUser } from "@/components/nav-user"
import { useAuth } from "@/lib/auth"
import {
  Sidebar,
  SidebarContent,
  SidebarFooter,
  SidebarHeader,
} from "@/components/ui/sidebar"
import { Suspense } from "react"

export function AppSidebar(props: React.ComponentProps<typeof Sidebar>) {
  const { session } = useAuth()

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
