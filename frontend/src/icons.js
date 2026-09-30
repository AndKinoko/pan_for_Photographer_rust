/* ===========================================================================
   图标注册表
   ---------------------------------------------------------------------------
   全站图形语义统一走 Lucide（https://lucide.dev），不再用 emoji。
   原先 emoji 的三个实际问题：
     1. 同一份代码在 Windows / macOS / Android 上渲染出三套字形；
     2. 颜色由字体决定，`color` / 主题令牌对它们无效，暗色下亮度也不可调；
     3. 无法 `aria-hidden`，装饰性图形会污染无障碍树（屏幕阅读器把 "🖼️" 念成
        "framed picture"）。
   Lucide 图标默认 `stroke="currentColor"`，所以自动跟随 `color` 与明暗主题。

   这里全部用**具名静态导入**。不要改成 `import * as icons` 再按字符串查表——
   那样 Vite 无法做 tree-shaking，会把 6000+ 个图标全打进产物。
   新增图标：在这一行加导入，再补进 ICONS 即可。
   =========================================================================== */

import {
  // 导航与品牌
  Camera,
  Files,
  Folder,
  FolderOpen,
  FolderPlus,
  FolderInput,
  Search,
  SearchX,
  Link,
  Trash2,
  Settings,
  Package,
  // 方向与移动
  Upload,
  Download,
  ChevronLeft,
  ChevronRight,
  ChevronDown,
  // 操作
  Pencil,
  RotateCcw,
  Copy,
  CheckSquare,
  Square,
  X,
  Repeat,
  Plus,
  Menu,
  MoreHorizontal,
  // 文件类型（fileIcon() 的返回值即这里的键名）
  Image,
  Film,
  Music,
  FileText,
  FileSpreadsheet,
  Presentation,
  FileArchive,
  Code,
  File,
  // 状态
  Check,
  CircleAlert,
  CircleX,
  TriangleAlert,
  Info,
  Clock,
  Ban,
  // 主题与密码可见性
  Sun,
  Moon,
  Eye,
  EyeOff,
  // 其它
  Lock,
  Users,
  HardDrive,
} from '@lucide/vue'

export const ICONS = {
  Camera,
  Files,
  Folder,
  FolderOpen,
  FolderPlus,
  FolderInput,
  Search,
  SearchX,
  Link,
  Trash2,
  Settings,
  Package,
  Upload,
  Download,
  ChevronLeft,
  ChevronRight,
  ChevronDown,
  Pencil,
  RotateCcw,
  Copy,
  CheckSquare,
  Square,
  X,
  Repeat,
  Plus,
  Menu,
  MoreHorizontal,
  Image,
  Film,
  Music,
  FileText,
  FileSpreadsheet,
  Presentation,
  FileArchive,
  Code,
  File,
  Check,
  CircleAlert,
  CircleX,
  TriangleAlert,
  Info,
  Clock,
  Ban,
  Sun,
  Moon,
  Eye,
  EyeOff,
  Lock,
  Users,
  HardDrive,
}

/** 图标名是否已注册（开发期自检用）。 */
export const hasIcon = (name) => Object.prototype.hasOwnProperty.call(ICONS, name)
