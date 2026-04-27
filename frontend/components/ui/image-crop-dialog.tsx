import * as React from "react";
import { useCallback, useEffect, useState } from "react";
import CropperImpl, { type Area, type CropperProps } from "react-easy-crop";

// react-easy-crop is typed as a class component, which trips React 19's
// stricter JSX checking. defaultProps fill in everything we don't pass, so a
// Partial<CropperProps> shape is accurate at runtime.
const Cropper = CropperImpl as unknown as React.ComponentType<Partial<CropperProps>>;
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { LoaderIcon } from "lucide-react";

interface ImageCropDialogProps {
  open: boolean;
  file: File | null;
  /** Output bitmap edge length in pixels. */
  outputSize?: number;
  onCancel: () => void;
  onConfirm: (cropped: File) => void;
}

export function ImageCropDialog({
  open,
  file,
  outputSize = 512,
  onCancel,
  onConfirm,
}: ImageCropDialogProps) {
  const [src, setSrc] = useState<string | null>(null);
  const [crop, setCrop] = useState({ x: 0, y: 0 });
  const [zoom, setZoom] = useState(1);
  const [areaPx, setAreaPx] = useState<Area | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    if (!file) {
      setSrc(null);
      return;
    }
    const url = URL.createObjectURL(file);
    setSrc(url);
    setCrop({ x: 0, y: 0 });
    setZoom(1);
    setAreaPx(null);
    return () => URL.revokeObjectURL(url);
  }, [file]);

  const handleConfirm = useCallback(async () => {
    if (!src || !areaPx || !file) return;
    setBusy(true);
    try {
      const blob = await cropToBlob(src, areaPx, outputSize);
      const cropped = new File([blob], renameToWebp(file.name), {
        type: "image/webp",
        lastModified: Date.now(),
      });
      onConfirm(cropped);
    } finally {
      setBusy(false);
    }
  }, [src, areaPx, file, outputSize, onConfirm]);

  return (
    <Dialog
      open={open && !!file}
      onOpenChange={(next: boolean) => {
        if (!next && !busy) onCancel();
      }}
    >
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>Crop image</DialogTitle>
        </DialogHeader>

        <div className="relative aspect-square w-full overflow-hidden rounded-md bg-muted">
          {src && (
            <Cropper
              image={src}
              crop={crop}
              zoom={zoom}
              aspect={1}
              minZoom={0.5}
              maxZoom={4}
              cropShape="rect"
              showGrid={false}
              objectFit="contain"
              restrictPosition={false}
              onCropChange={setCrop}
              onZoomChange={setZoom}
              onCropComplete={(_area: Area, pixels: Area) => setAreaPx(pixels)}
            />
          )}
        </div>

        <div className="flex items-center gap-3 px-1">
          <span className="text-xs font-medium text-muted-foreground">Zoom</span>
          <input
            type="range"
            min={0.5}
            max={4}
            step={0.01}
            value={zoom}
            onChange={(e) => setZoom(Number(e.target.value))}
            className="h-1 flex-1 cursor-pointer appearance-none rounded-full bg-muted accent-primary"
          />
        </div>

        <DialogFooter>
          <Button variant="outline" onClick={onCancel} disabled={busy}>
            Cancel
          </Button>
          <Button onClick={handleConfirm} disabled={busy || !areaPx}>
            {busy ? (
              <span className="flex items-center gap-2">
                <LoaderIcon className="h-3.5 w-3.5 animate-spin" />
                Processing
              </span>
            ) : (
              "Apply"
            )}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

async function cropToBlob(
  src: string,
  area: Area,
  outputSize: number,
): Promise<Blob> {
  const img = await loadImage(src);
  const canvas = document.createElement("canvas");
  canvas.width = outputSize;
  canvas.height = outputSize;
  const ctx = canvas.getContext("2d");
  if (!ctx) throw new Error("canvas 2d context unavailable");
  ctx.imageSmoothingEnabled = true;
  ctx.imageSmoothingQuality = "high";
  ctx.drawImage(
    img,
    area.x,
    area.y,
    area.width,
    area.height,
    0,
    0,
    outputSize,
    outputSize,
  );
  return await new Promise<Blob>((resolve, reject) => {
    canvas.toBlob(
      (b) => (b ? resolve(b) : reject(new Error("canvas.toBlob returned null"))),
      "image/webp",
      0.9,
    );
  });
}

function loadImage(src: string): Promise<HTMLImageElement> {
  return new Promise((resolve, reject) => {
    const img = new Image();
    img.onload = () => resolve(img);
    img.onerror = () => reject(new Error(`failed to load image: ${src}`));
    img.src = src;
  });
}

function renameToWebp(name: string): string {
  const dot = name.lastIndexOf(".");
  const base = dot > 0 ? name.slice(0, dot) : name;
  return `${base}.webp`;
}
