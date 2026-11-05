import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ContactAddressField, EmailBadgeField, ContactLinksField } from "./ContactAddressField";

afterEach(cleanup);
describe("email address paste cleanup", () => {
  it("cleans invitee addresses and ignores additional recipients in link headers", () => {
    const change = vi.fn();
    render(<EmailBadgeField label="Invitees" values={[]} onChange={change} />);
    fireEvent.paste(screen.getByRole("textbox", { name: "Invitees" }), { clipboardData: { getData: () => "mailto:jane+news@example.com?cc=hidden@example.com" } });
    expect(change).toHaveBeenCalledWith(["jane+news@example.com"]);
  });

  it("leaves pasted links to the link field", () => {
    const change = vi.fn();
    render(<ContactLinksField links={[]} onChange={change} />);
    const paste = new Event("paste", { bubbles: true, cancelable: true });
    Object.defineProperty(paste, "clipboardData", { value: { getData: () => "mailto:jane@example.com?subject=Hello" } });
    fireEvent(screen.getByRole("textbox"), paste);
    expect(paste.defaultPrevented).toBe(false);
    expect(change).not.toHaveBeenCalled();
  });

  it("cleans a copied link in contact addresses", () => {
    const change = vi.fn();
    render(<ContactAddressField addresses={[]} onChange={change} />);
    fireEvent.paste(screen.getByRole("textbox", { name: "Email Addresses" }), { clipboardData: { getData: () => "mailto:jane@example.com?subject=Hello" } });
    expect(change).toHaveBeenCalledWith(["jane@example.com"]);
  });
});
