export type NodeKind = "directory" | "file";

export type Node = {
  id: string;
  owner_id: string;
  parent_id: string | null;
  name: string;
  kind: NodeKind;
  size: number;
  etag: string;
  created_at: string;
  updated_at: string;
  deleted_at?: string;
};
