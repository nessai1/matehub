// Light-weight context so both sidebars (left AppSidebar, right
// MemberSidebar) can pop the same "Add Teammates" dialog without
// duplicating dialog state or wiring props through the layout tree.

import { createContext, useCallback, useContext, useState } from "react";
import { AddTeammatesDialog } from "@/components/hub/add-teammates-dialog";

interface Ctx {
  open: () => void;
}

const AddTeammatesContext = createContext<Ctx>({ open: () => {} });

export function AddTeammatesProvider({ children }: { children: React.ReactNode }) {
  const [isOpen, setIsOpen] = useState(false);
  const open = useCallback(() => setIsOpen(true), []);

  return (
    <AddTeammatesContext.Provider value={{ open }}>
      {children}
      <AddTeammatesDialog open={isOpen} onOpenChange={setIsOpen} />
    </AddTeammatesContext.Provider>
  );
}

export function useAddTeammates() {
  return useContext(AddTeammatesContext);
}
