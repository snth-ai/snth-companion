import { faceEn, type FaceStrings } from "./i18n.en";
import { faceRu } from "./i18n.ru";
import type { FacePreset } from "./schema";

export type { FaceStrings };

export function faceStrings(lang: string): FaceStrings {
  return lang === "ru" ? faceRu : faceEn;
}

export function presetTitle(p: FacePreset, lang: string): string {
  return lang === "ru" ? p.title_ru : p.title_en;
}

export function presetBlurb(p: FacePreset, lang: string): string {
  return lang === "ru" ? p.blurb_ru : p.blurb_en;
}
