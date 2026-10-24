import { createContext, useContext } from "react";
import { en, type Strings } from "./en";
import { ru } from "./ru";

export type Lang = "en" | "ru";
export type { Strings };

const dictionaries: Record<Lang, Strings> = { en, ru };

/** The language of the operating system, as far as the webview knows. */
export function systemLang(): Lang {
  const prefs = navigator.languages?.length ? navigator.languages : [navigator.language];
  for (const tag of prefs) {
    const short = tag.toLowerCase().slice(0, 2);
    if (short === "ru") return "ru";
    if (short === "en") return "en";
  }
  return "en";
}

/** The owner's choice if there is one, else the system language. */
export function resolveLang(choice: string | null | undefined): Lang {
  return choice === "ru" || choice === "en" ? choice : systemLang();
}

export function stringsFor(lang: Lang): Strings {
  return dictionaries[lang];
}

export interface I18n {
  lang: Lang;
  s: Strings;
}

export const I18nContext = createContext<I18n>({ lang: "en", s: en });

export function useI18n(): I18n {
  return useContext(I18nContext);
}
