import { useEffect, useState } from "react";

/** True while the media query matches; environments without matchMedia count as a wide screen. */
export function useMedia(query: string): boolean {
  const get = () => (typeof window.matchMedia === "function" ? window.matchMedia(query).matches : true);
  const [matches, setMatches] = useState(get);
  useEffect(() => {
    if (typeof window.matchMedia !== "function") return;
    const list = window.matchMedia(query);
    const onChange = () => setMatches(list.matches);
    onChange();
    list.addEventListener("change", onChange);
    return () => list.removeEventListener("change", onChange);
  }, [query]);
  return matches;
}

export const useDesktop = () => useMedia("(min-width: 1024px)");
