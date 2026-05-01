import { useCallback, useEffect, useRef, useState } from "react";
import { CameraIcon, LoaderIcon } from "lucide-react";
import { Avatar, AvatarFallback, AvatarImage } from "@/components/ui/avatar";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { ImageCropDialog } from "@/components/ui/image-crop-dialog";
import { useAuth } from "@/lib/auth";
import { patchHub, uploadHubAvatar, useHub } from "@/hooks/use-hub";
import { t } from "@/i18n";

interface Props {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

interface ServiceInfo {
  name: string;
  version: string | null;
  status: "up" | "down";
}

// Admin-only dialog for editing hub identity. The General Settings entry
// in HubSwitcher is gated on perms?.is_admin so unauthorised users never
// see this; the backend rejects with 403 either way.
export function HubSettingsDialog({ open, onOpenChange }: Props) {
  const { session } = useAuth();
  const { hub, mutate } = useHub();

  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [avatarPreview, setAvatarPreview] = useState<string | null>(null);
  const [avatarFile, setAvatarFile] = useState<File | null>(null);
  const [cropSource, setCropSource] = useState<File | null>(null);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");
  const [services, setServices] = useState<ServiceInfo[] | null>(null);

  const fileInputRef = useRef<HTMLInputElement>(null);

  // Lazy-fetch service versions only when the dialog opens. Re-fetch on
  // each open so the operator sees fresh data after a deploy without
  // reloading the whole SPA.
  useEffect(() => {
    if (!open || !session?.token) return;
    let cancelled = false;
    setServices(null);
    fetch(`/api/hub/v1/services/versions`, {
      headers: { Authorization: `Bearer ${session.token}` },
    })
      .then((r) => (r.ok ? r.json() : []))
      .then((data: ServiceInfo[]) => {
        if (!cancelled) setServices(data);
      })
      .catch(() => {
        if (!cancelled) setServices([]);
      });
    return () => {
      cancelled = true;
    };
  }, [open, session?.token]);

  // Re-seed draft state every time the dialog opens — covers both the
  // first open and repeat opens after another tab edited the hub.
  useEffect(() => {
    if (open && hub) {
      setName(hub.name);
      setDescription(hub.description ?? "");
      setAvatarPreview(null);
      setAvatarFile(null);
      setCropSource(null);
      setError("");
    }
  }, [open, hub]);

  const handlePick = useCallback(
    (e: React.ChangeEvent<HTMLInputElement>) => {
      const file = e.target.files?.[0];
      if (e.target) e.target.value = "";
      if (!file) return;
      if (file.size > 10 * 1024 * 1024) {
        setError(t("Image must be under 10MB"));
        return;
      }
      setError("");
      setCropSource(file);
    },
    [],
  );

  const handleCropConfirm = useCallback((cropped: File) => {
    setAvatarFile(cropped);
    setCropSource(null);
    const reader = new FileReader();
    reader.onload = (ev) => setAvatarPreview(ev.target?.result as string);
    reader.readAsDataURL(cropped);
  }, []);

  const handleSave = async () => {
    if (!session || !hub) return;
    const trimmed = name.trim();
    if (!trimmed) {
      setError(t("Hub name is required"));
      return;
    }

    setSaving(true);
    setError("");

    if (avatarFile) {
      const result = await uploadHubAvatar(session.hubId, session.token, avatarFile);
      if ("error" in result) {
        setError(
          result.status === 403
            ? t("Only hub admins can change settings")
            : `${t("Avatar upload failed")} (${result.status})`,
        );
        setSaving(false);
        return;
      }
    }

    const nameChanged = trimmed !== hub.name;
    const descChanged = (description || null) !== (hub.description || null);
    if (nameChanged || descChanged) {
      const result = await patchHub(session.hubId, session.token, {
        name: nameChanged ? trimmed : undefined,
        description: descChanged ? description : undefined,
      });
      if ("error" in result) {
        setError(
          result.status === 403
            ? t("Only hub admins can change settings")
            : `${t("Save failed")} (${result.status})`,
        );
        setSaving(false);
        return;
      }
    }

    await mutate();
    setSaving(false);
    onOpenChange(false);
  };

  const initials = (hub?.name ?? "?").charAt(0).toUpperCase();
  const currentAvatar = avatarPreview || hub?.avatar_url || null;

  return (
    <>
      <Dialog open={open} onOpenChange={onOpenChange}>
        <DialogContent className="sm:max-w-md">
          <DialogHeader>
            <DialogTitle>{t("Hub Settings")}</DialogTitle>
          </DialogHeader>

          <div className="flex flex-col items-center gap-4 py-2">
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
                onChange={handlePick}
              />
            </button>

            <p className="text-xs text-muted-foreground">
              {t("Click to upload icon (max 10MB)")}
            </p>

            <div className="w-full space-y-2">
              <label className="text-sm font-medium">{t("Hub name")}</label>
              <Input
                value={name}
                onChange={(e) => setName(e.target.value)}
                placeholder={t("e.g. Acme Team")}
                maxLength={80}
              />
            </div>

            <div className="w-full space-y-2">
              <label className="text-sm font-medium">{t("Description")}</label>
              <textarea
                value={description}
                onChange={(e) => setDescription(e.target.value)}
                placeholder={t("What's this hub about?")}
                rows={3}
                maxLength={500}
                className="w-full resize-none rounded-md border bg-transparent px-3 py-2 text-sm outline-none placeholder:text-muted-foreground focus:ring-1 focus:ring-primary"
              />
              <p className="text-[11px] text-muted-foreground">
                {description.length}/500
              </p>
            </div>

            <div className="w-full space-y-2">
              <label className="text-sm font-medium">{t("Services")}</label>
              <div className="rounded-md border bg-muted/30 px-3 py-2">
                {services === null ? (
                  <p className="text-xs text-muted-foreground">
                    {t("Loading...")}
                  </p>
                ) : services.length === 0 ? (
                  <p className="text-xs text-muted-foreground">
                    {t("No services reachable")}
                  </p>
                ) : (
                  <ul className="space-y-1">
                    {services.map((s) => (
                      <li
                        key={s.name}
                        className="flex items-center gap-2 text-xs"
                      >
                        <span
                          className={
                            s.status === "up"
                              ? "size-1.5 shrink-0 rounded-full bg-emerald-500"
                              : "size-1.5 shrink-0 rounded-full bg-zinc-500"
                          }
                          aria-hidden
                        />
                        <span className="font-medium">{s.name}</span>
                        <span className="ml-auto font-mono text-[10px] text-muted-foreground">
                          {s.version ?? "—"}
                        </span>
                      </li>
                    ))}
                  </ul>
                )}
              </div>
            </div>

            {error && (
              <p className="w-full rounded border border-destructive/30 bg-destructive/10 px-3 py-2 text-xs text-destructive">
                {error}
              </p>
            )}
          </div>

          <DialogFooter>
            <Button variant="outline" onClick={() => onOpenChange(false)}>
              {t("Cancel")}
            </Button>
            <Button onClick={handleSave} disabled={saving || !hub}>
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
  );
}
