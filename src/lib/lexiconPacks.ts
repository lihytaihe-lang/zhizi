import { isTauriRuntime } from "./tauriRuntime";

export type PackFile = { file: string; sha256: string; bytes: number };

export type PackEntry = {
  id: string;
  name: string;
  description: string;
  rows: number;
  dict: PackFile;
  membership: PackFile;
};

export type PackIndex = {
  formatVersion: number;
  lexiconVersion: string;
  packs: PackEntry[];
};

export type InstalledPack = {
  id: string;
  membershipBytes: number;
  dictPresent: boolean;
};

async function invokeLexiconPacks<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  if (!isTauriRuntime()) {
    throw new Error("此功能需要在知字桌面管理台中使用。");
  }
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke<T>(command, args);
}

export const lexiconPacksAvailable = (tag?: string) =>
  invokeLexiconPacks<PackIndex>("lexicon_packs_available", tag === undefined ? undefined : { tag });
export const lexiconPacksInstalled = () =>
  invokeLexiconPacks<InstalledPack[]>("lexicon_packs_installed");
export const lexiconPackInstall = (id: string, tag?: string) =>
  invokeLexiconPacks<InstalledPack>("lexicon_pack_install", tag === undefined ? { id } : { id, tag });
export const lexiconPackRemove = (id: string) =>
  invokeLexiconPacks<InstalledPack[]>("lexicon_pack_remove", { id });
