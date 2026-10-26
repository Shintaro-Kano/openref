<script lang="ts">
  /* ラベルのバッジ（Paperpile の [liver]）。色は labels.ts のパレット（背景 + 文字 + 枠）。
     サイドバー・一覧の行・ピッカー・詳細・Manage labels で共通。class は lbadge（汎用の .lbl は ReferenceEditor の項目名が使用済み） */
  import { isSolid, labelVars } from "./labels";

  let { name, color = null }: { name: string; color?: string | null } = $props();
</script>

<span class="lbadge" class:solid={isSolid(color)} style={labelVars(color)}>{name}</span>
