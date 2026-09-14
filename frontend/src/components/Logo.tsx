import React from "react";
import Image from "next/image";

interface LogoProps {
    isCollapsed: boolean;
}

// 品牌位只做展示：以前点它会弹出「关于」，用户反馈这个页面没用，入口已隐藏
const Logo = React.forwardRef<HTMLDivElement, LogoProps>(({ isCollapsed }, ref) => (
  isCollapsed ? (
    <div ref={ref} className="flex items-center justify-start mb-2">
      <Image src="/logo-collapsed.png" alt="Meetily" width={40} height={32} />
    </div>
  ) : (
    <div
      ref={ref}
      className="w-full text-lg text-center border rounded-full bg-blue-50 border-white font-semibold text-gray-700 mb-2 block items-center"
    >
      <span>Meetily</span>
    </div>
  )
));

Logo.displayName = "Logo";

export default Logo;
