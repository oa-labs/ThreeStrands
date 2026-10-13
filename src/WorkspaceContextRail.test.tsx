import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { WorkspaceContextRail } from "./WorkspaceContextRail";

describe("shared workspace context rail",()=>{
  afterEach(()=>{cleanup();localStorage.clear();});
  it("uses the email framing and retains editable content while collapsed",async()=>{
    render(<WorkspaceContextRail label="Task context"><input aria-label="Notes" defaultValue="Draft" /></WorkspaceContextRail>);
    const input=screen.getByRole("textbox",{name:"Notes"});
    expect(screen.getByRole("complementary",{name:"Task context"})).toHaveClass("context-panel");
    fireEvent.change(input,{target:{value:"Unsaved text"}});
    fireEvent.click(screen.getByRole("button",{name:"Hide Task context"}));
    expect(screen.queryByRole("textbox",{name:"Notes"})).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button",{name:"Show Task context"}));
    expect(screen.getByRole("textbox",{name:"Notes"})).toHaveValue("Unsaved text");
    fireEvent.click(screen.getByRole("button",{name:"Hide Task context"}));
    window.dispatchEvent(new CustomEvent("threestrands:focus-context",{detail:"Task context"}));
    await waitFor(()=>expect(input).toHaveFocus());
  });
});
