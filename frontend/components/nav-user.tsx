import { useCallback, useRef, useState } from "react"
import { useNavigate } from "react-router"
import { useAuth, type AuthSession } from "@/lib/auth"
import {
  Avatar,
  AvatarFallback,
  AvatarImage,
} from "@/components/ui/avatar"
import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu"
import { Input } from "@/components/ui/input"
import { ImageCropDialog } from "@/components/ui/image-crop-dialog"
import {
  SidebarMenu,
  SidebarMenuButton,
  SidebarMenuItem,
  useSidebar,
} from "@/components/ui/sidebar"
import {
  ChevronsUpDownIcon,
  UserIcon,
  LogOutIcon,
  CameraIcon,
  LoaderIcon,
  LanguagesIcon,
  SunIcon,
  MoonIcon,
  MonitorIcon,
  PaletteIcon,
} from "lucide-react"
import { useTheme } from "next-themes"
import { t, useLocale, LOCALES } from "@/i18n"

const HUB_API = "/api/hub"

export function NavUser({
  user,
}: {
  user: {
    name: string
    username: string
    role?: string
  }
}) {
  const { isMobile } = useSidebar()
  const { session, login, logout } = useAuth()
  const navigate = useNavigate()
  const [editOpen, setEditOpen] = useState(false)
  const [savedName, setSavedName] = useState(session?.displayName ?? user.name)
  const [draftName, setDraftName] = useState(savedName)
  const [avatarUrl, setAvatarUrl] = useState<string | null>(session?.avatarUrl ?? null)
  const [avatarPreview, setAvatarPreview] = useState<string | null>(null)
  const [avatarFile, setAvatarFile] = useState<File | null>(null)
  const [cropSource, setCropSource] = useState<File | null>(null)
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState("")
  const fileInputRef = useRef<HTMLInputElement>(null)

  const handleAvatarChange = useCallback(
    (e: React.ChangeEvent<HTMLInputElement>) => {
      const file = e.target.files?.[0]
      // Allow re-picking the same file: clear input value
      if (e.target) e.target.value = ""
      if (!file) return

      // Pre-crop limit: 10MB, generous enough for phone photos.
      // The cropped output is ~50KB webp, well under the 2MB server limit.
      if (file.size > 10 * 1024 * 1024) {
        setError(t("Image must be under 10MB"))
        return
      }

      setError("")
      setCropSource(file)
    },
    [],
  )

  const handleCropConfirm = useCallback((cropped: File) => {
    setAvatarFile(cropped)
    setCropSource(null)
    const reader = new FileReader()
    reader.onload = (ev) => {
      setAvatarPreview(ev.target?.result as string)
    }
    reader.readAsDataURL(cropped)
  }, [])

  const handleSave = useCallback(async () => {
    if (!session) return
    setSaving(true)
    setError("")

    try {
      let updatedSession: AuthSession | null = null;

      const authHeaders = { Authorization: `Bearer ${session.token}` }

      // Upload avatar if changed
      if (avatarFile) {
        const formData = new FormData()
        formData.append("avatar", avatarFile)

        const res = await fetch(
          `${HUB_API}/v1/hubs/${session.hubId}/profile/avatar`,
          {
            method: "POST",
            headers: authHeaders,
            body: formData,
          },
        )

        if (!res.ok) {
          const status = res.status
          if (status === 413) setError(t("Image too large (max 2MB)"))
          else if (status === 415) setError(t("Unsupported image format"))
          else setError(`Upload failed (${status})`)
          setSaving(false)
          return
        }

        const { url } = await res.json()
        setAvatarUrl(url)
        setAvatarPreview(null)
        setAvatarFile(null)

        updatedSession = { ...(updatedSession ?? session), avatarUrl: url }
      }

      // Update display name on backend
      if (draftName !== savedName) {
        const res = await fetch(
          `${HUB_API}/v1/hubs/${session.hubId}/profile`,
          {
            method: "PATCH",
            headers: { ...authHeaders, "Content-Type": "application/json" },
            body: JSON.stringify({ display_name: draftName }),
          },
        )

        if (!res.ok) {
          setError(`Failed to update name (${res.status})`)
          setSaving(false)
          return
        }
      }

      // Persist to local session
      updatedSession = {
        ...(updatedSession ?? session),
        displayName: draftName,
      }
      login(updatedSession)
      setSavedName(draftName)

      setEditOpen(false)
    } catch {
      setError(t("Cannot reach server"))
    } finally {
      setSaving(false)
    }
  }, [session, avatarFile, draftName, savedName, login])

  const currentAvatar = avatarPreview || avatarUrl
  const initials = savedName.charAt(0).toUpperCase()

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
                <Avatar className="h-8 w-8">
                  {currentAvatar && <AvatarImage src={currentAvatar} />}
                  <AvatarFallback className="bg-primary/15 text-xs font-medium text-primary">
                    {initials}
                  </AvatarFallback>
                </Avatar>
                <div className="grid flex-1 text-left text-sm leading-tight">
                  <span className="truncate font-medium">{savedName}</span>
                  <span className="truncate text-xs text-muted-foreground">
                    @{user.username}
                  </span>
                </div>
                <ChevronsUpDownIcon className="ml-auto size-4" />
              </SidebarMenuButton>
            </DropdownMenuTrigger>
            <DropdownMenuContent
              className="w-(--radix-dropdown-menu-trigger-width) min-w-56 rounded-lg"
              side={isMobile ? "bottom" : "right"}
              align="end"
              sideOffset={4}
            >
              <DropdownMenuLabel className="p-0 font-normal">
                <div className="flex items-center gap-2 px-1 py-1.5 text-left text-sm">
                  <Avatar className="h-8 w-8">
                    {currentAvatar && <AvatarImage src={currentAvatar} />}
                    <AvatarFallback className="bg-primary/15 text-xs font-medium text-primary">
                      {initials}
                    </AvatarFallback>
                  </Avatar>
                  <div className="grid flex-1 text-left text-sm leading-tight">
                    <span className="truncate font-medium">{savedName}</span>
                    <span className="truncate text-xs text-muted-foreground">
                      @{user.username}
                      {user.role && (
                        <span className="ml-1.5 rounded bg-primary/10 px-1 py-0.5 text-[10px] text-primary">
                          {user.role}
                        </span>
                      )}
                    </span>
                  </div>
                </div>
              </DropdownMenuLabel>
              <DropdownMenuSeparator />
              <DropdownMenuItem onSelect={() => setEditOpen(true)}>
                <UserIcon />
                {t("Profile")}
              </DropdownMenuItem>
              <LanguageMenu />
              <ThemeMenu />
              <DropdownMenuSeparator />
              <DropdownMenuItem
                className="text-destructive focus:text-destructive"
                onSelect={() => {
                  logout()
                  navigate("/login", { replace: true })
                }}
              >
                <LogOutIcon />
                {t("Log out")}
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
        </SidebarMenuItem>
      </SidebarMenu>

      <Dialog
        open={editOpen}
        onOpenChange={(open: boolean) => {
          setEditOpen(open)
          // Always reset draft to saved values (both on open and close)
          setDraftName(savedName)
          setAvatarPreview(null)
          setAvatarFile(null)
          setCropSource(null)
          setError("")
        }}
      >
        <DialogContent className="sm:max-w-sm">
          <DialogHeader>
            <DialogTitle>{t("Edit Profile")}</DialogTitle>
          </DialogHeader>

          <div className="flex flex-col items-center gap-4 py-4">
            {/* Avatar upload */}
            <button
              type="button"
              onClick={() => fileInputRef.current?.click()}
              className="group relative"
            >
              <Avatar className="h-20 w-20">
                {currentAvatar && <AvatarImage src={currentAvatar} />}
                <AvatarFallback className="bg-primary/15 text-2xl font-bold text-primary">
                  {initials}
                </AvatarFallback>
              </Avatar>
              <div className="absolute inset-0 flex items-center justify-center rounded-md bg-black/50 opacity-0 transition-opacity group-hover:opacity-100">
                <CameraIcon className="h-5 w-5 text-white" />
              </div>
              <input
                ref={fileInputRef}
                type="file"
                accept="image/png,image/jpeg,image/webp,image/gif"
                className="hidden"
                onChange={handleAvatarChange}
              />
            </button>

            <p className="text-xs text-muted-foreground">
              {t("Click to upload avatar (max 10MB)")}
            </p>

            {/* Display name */}
            <div className="w-full space-y-2">
              <label className="text-sm font-medium">{t("Display Name")}</label>
              <Input
                value={draftName}
                onChange={(e) => setDraftName(e.target.value)}
                placeholder={t("Your display name")}
              />
            </div>

            {/* Username (read-only) */}
            <div className="w-full space-y-2">
              <label className="text-sm font-medium text-muted-foreground">
                {t("Username")}
              </label>
              <Input value={`@${user.username}`} disabled />
            </div>

            {error && (
              <p className="w-full rounded border border-destructive/30 bg-destructive/10 px-3 py-2 text-xs text-destructive">
                {error}
              </p>
            )}
          </div>

          <DialogFooter>
            <Button variant="outline" onClick={() => setEditOpen(false)}>
              {t("Cancel")}
            </Button>
            <Button onClick={handleSave} disabled={saving}>
              {saving ? (
                <span className="flex items-center gap-2">
                  <LoaderIcon className="h-3.5 w-3.5 animate-spin" />
                  {t("Saving")}
                </span>
              ) : (
                t("Save")
              )}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <ImageCropDialog
        open={cropSource !== null}
        file={cropSource}
        onCancel={() => setCropSource(null)}
        onConfirm={handleCropConfirm}
      />
    </>
  )
}

// ── Language picker ───────────────────────────────────────────
// Submenu with one radio item per supported locale. Selection writes to
// localStorage and reloads the page (same flow Superset uses) so the
// translator hydrates with the chosen pack before any t() runs again.
function LanguageMenu() {
  const { locale, setLocale } = useLocale()
  return (
    <DropdownMenuSub>
      <DropdownMenuSubTrigger>
        <LanguagesIcon />
        {t("Language")}
      </DropdownMenuSubTrigger>
      <DropdownMenuSubContent>
        <DropdownMenuRadioGroup
          value={locale}
          onValueChange={(v) => setLocale(v as typeof locale)}
        >
          {LOCALES.map((l) => (
            <DropdownMenuRadioItem key={l.code} value={l.code}>
              {l.nativeLabel}
            </DropdownMenuRadioItem>
          ))}
        </DropdownMenuRadioGroup>
      </DropdownMenuSubContent>
    </DropdownMenuSub>
  )
}

// ── Theme picker ──────────────────────────────────────────────
// Submenu mirroring the language one. next-themes keeps the choice in
// localStorage and applies the `class="dark"` (or scheme attr) on <html>
// — no reload needed, the document re-styles immediately.
function ThemeMenu() {
  const { theme, setTheme } = useTheme()
  // theme can be undefined on first paint; fall back to "system" so the
  // radio group always has a value.
  const current = theme ?? "system"
  return (
    <DropdownMenuSub>
      <DropdownMenuSubTrigger>
        <PaletteIcon />
        {t("Theme")}
      </DropdownMenuSubTrigger>
      <DropdownMenuSubContent>
        <DropdownMenuRadioGroup value={current} onValueChange={setTheme}>
          <DropdownMenuRadioItem value="light">
            <SunIcon className="mr-2 h-3.5 w-3.5" />
            {t("Light")}
          </DropdownMenuRadioItem>
          <DropdownMenuRadioItem value="dark">
            <MoonIcon className="mr-2 h-3.5 w-3.5" />
            {t("Dark")}
          </DropdownMenuRadioItem>
          <DropdownMenuRadioItem value="system">
            <MonitorIcon className="mr-2 h-3.5 w-3.5" />
            {t("System")}
          </DropdownMenuRadioItem>
        </DropdownMenuRadioGroup>
      </DropdownMenuSubContent>
    </DropdownMenuSub>
  )
}
