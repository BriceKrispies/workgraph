import { render } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { Markdown } from "../src/components/Markdown";

describe("Markdown", () => {
  it("renders headings, lists, code and emphasis", () => {
    const fence = "```";
    const source = `# Title\n\n- one\n- **two**\n\n${fence}\ncode()\n${fence}\n\nuse \`x\` now`;
    const { container } = render(<Markdown source={source} />);
    expect(container.querySelector("h1")?.textContent).toBe("Title");
    expect(container.querySelectorAll("li")).toHaveLength(2);
    expect(container.querySelector("strong")?.textContent).toBe("two");
    expect(container.querySelector("pre code")?.textContent).toBe("code()");
  });

  it("never interprets raw HTML", () => {
    const { container } = render(<Markdown source={'<img src=x onerror="alert(1)"><script>alert(2)</script> hello'} />);
    expect(container.querySelector("img")).toBeNull();
    expect(container.querySelector("script")).toBeNull();
    expect(container.textContent).toContain("<script>alert(2)</script>");
  });

  it("only links safe URLs", () => {
    const { container } = render(<Markdown source={"[ok](https://example.com) [bad](javascript:alert(1))"} />);
    const links = container.querySelectorAll("a");
    expect(links).toHaveLength(1);
    expect(links[0].getAttribute("href")).toBe("https://example.com");
    expect(links[0].getAttribute("rel")).toContain("noopener");
  });
});
