"use client"

import { useState } from "react"
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu"
import {
  SidebarMenu,
  SidebarMenuButton,
  SidebarMenuItem,
  useSidebar,
} from "@/components/ui/sidebar"
import {
  ChevronsUpDownIcon,
  SettingsIcon,
  LinkIcon,
  UsersIcon,
  ShieldIcon,
} from "lucide-react"
import { GroupsRolesDialog } from "@/components/hub/groups-roles-dialog"
import { useMembers } from "@/hooks/use-members"

interface Hub {
  id: string
  name: string
  slug: string
  plan: string
}

export function HubSwitcher({
  currentHub,
  otherHubs,
}: {
  currentHub: Hub
  otherHubs: Hub[]
}) {
  const { isMobile } = useSidebar()
  const { refetch: refetchMembers } = useMembers()
  const [rolesOpen, setRolesOpen] = useState(false)

  return (
    <>
      <SidebarMenu>
        <SidebarMenuItem>
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <SidebarMenuButton
                size="lg"
                className="data-[state=open]:bg-sidebar-accent data-[state=open]:text-sidebar-accent-foreground"
              >
                <div className="flex aspect-square size-8 items-center justify-center rounded-md bg-sidebar-primary text-sidebar-primary-foreground">
                  <UsersIcon className="size-4" />
                </div>
                <div className="grid flex-1 text-left text-sm leading-tight">
                  <span className="truncate font-medium">{currentHub.name}</span>
                  <span className="truncate text-xs text-muted-foreground">
                    {currentHub.plan}
                  </span>
                </div>
                <ChevronsUpDownIcon className="ml-auto" />
              </SidebarMenuButton>
            </DropdownMenuTrigger>
            <DropdownMenuContent
              className="w-(--radix-dropdown-menu-trigger-width) min-w-56 rounded-lg"
              align="start"
              side={isMobile ? "bottom" : "right"}
              sideOffset={4}
            >
              <DropdownMenuLabel className="text-xs text-muted-foreground">
                Hub Settings
              </DropdownMenuLabel>
              <DropdownMenuItem className="gap-2">
                <SettingsIcon className="size-4 text-muted-foreground" />
                General
              </DropdownMenuItem>
              <DropdownMenuItem className="gap-2" onSelect={() => setRolesOpen(true)}>
                <ShieldIcon className="size-4 text-muted-foreground" />
                Groups & Roles
              </DropdownMenuItem>
              <DropdownMenuItem className="gap-2">
                <LinkIcon className="size-4 text-muted-foreground" />
                Invite Links
              </DropdownMenuItem>
              {otherHubs.length > 0 && (
                <>
                  <DropdownMenuSeparator />
                  <DropdownMenuLabel className="text-xs text-muted-foreground">
                    Other Hubs
                  </DropdownMenuLabel>
                  {otherHubs.map((hub) => (
                    <DropdownMenuItem key={hub.id} className="gap-2 p-2">
                      <div className="flex size-6 items-center justify-center rounded-md border text-xs font-medium">
                        {hub.name.charAt(0).toUpperCase()}
                      </div>
                      {hub.name}
                    </DropdownMenuItem>
                  ))}
                </>
              )}
            </DropdownMenuContent>
          </DropdownMenu>
        </SidebarMenuItem>
      </SidebarMenu>

      <GroupsRolesDialog open={rolesOpen} onOpenChange={setRolesOpen} onGroupsChanged={refetchMembers} />
    </>
  )
}
