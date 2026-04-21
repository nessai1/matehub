import { useCallback, useRef, useState } from "react";
import type { Attachment, ChatClient } from "@matehub/sdk-chat";

export interface PendingUpload {
  /** Local unique ID for this upload (separate from server attachment_id) */
  id: string;
  file: File;
  /** Preview URL (object URL for images/videos) */
  previewUrl: string;
  /** 0-100 progress, or null if not started */
  progress: number | null;
  /** Result from server, present when upload done */
  attachment: Attachment | null;
  error: string | null;
  abort: () => void;
}

/**
 * Manages in-flight file uploads for a chat message.
 * Users add files -> uploads start immediately -> when all done, message can be sent.
 */
export function useAttachmentUpload(
  client: ChatClient | null,
  channelId: number,
) {
  const [uploads, setUploads] = useState<PendingUpload[]>([]);
  const nextId = useRef(0);

  const updateUpload = useCallback(
    (id: string, patch: Partial<PendingUpload>) => {
      setUploads((prev) =>
        prev.map((u) => (u.id === id ? { ...u, ...patch } : u)),
      );
    },
    [],
  );

  const addFiles = useCallback(
    (files: FileList | File[]) => {
      if (!client) return;
      const fileArray = Array.from(files);
      for (const file of fileArray) {
        const localId = `upload-${nextId.current++}`;
        const previewUrl = URL.createObjectURL(file);
        const abortController = new AbortController();

        const upload: PendingUpload = {
          id: localId,
          file,
          previewUrl,
          progress: 0,
          attachment: null,
          error: null,
          abort: () => {
            abortController.abort();
            setUploads((prev) => prev.filter((u) => u.id !== localId));
            URL.revokeObjectURL(previewUrl);
          },
        };
        setUploads((prev) => [...prev, upload]);

        // Start upload
        client
          .uploadAttachment(channelId, file, {
            onProgress: (p) => updateUpload(localId, { progress: p }),
            signal: abortController.signal,
          })
          .then((attachment) => {
            updateUpload(localId, { attachment, progress: 100 });
          })
          .catch((err) => {
            if (err?.message === "Upload cancelled") return;
            updateUpload(localId, {
              error: err?.message ?? "Upload failed",
              progress: null,
            });
          });
      }
    },
    [client, channelId, updateUpload],
  );

  const removeUpload = useCallback((id: string) => {
    setUploads((prev) => {
      const found = prev.find((u) => u.id === id);
      if (found) {
        URL.revokeObjectURL(found.previewUrl);
      }
      return prev.filter((u) => u.id !== id);
    });
  }, []);

  const clearAll = useCallback(() => {
    setUploads((prev) => {
      for (const u of prev) URL.revokeObjectURL(u.previewUrl);
      return [];
    });
  }, []);

  /** Returns attachments ready to attach to a message (completed uploads). */
  const readyAttachments = uploads
    .filter((u) => u.attachment !== null)
    .map((u) => u.attachment as Attachment);

  const hasInFlight = uploads.some(
    (u) => u.attachment === null && u.error === null,
  );

  return {
    uploads,
    addFiles,
    removeUpload,
    clearAll,
    readyAttachments,
    hasInFlight,
  };
}
