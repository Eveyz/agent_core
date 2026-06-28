import { useEffect, useRef, useCallback, useState, useLayoutEffect } from 'react';

interface UseAutoScrollOptions {
  /** 当这些依赖变化时，如果处于自动滚动模式，立即贴底（用于新消息、切换会话等） */
  deps: any[];
  /** 是否为处理中状态——为 true 时启动 rAF 循环持续贴底 */
  isProcessing: boolean;
}

export function useAutoScroll<
  T extends HTMLElement,
  U extends HTMLElement = HTMLDivElement,
>(options: UseAutoScrollOptions) {
  const { deps, isProcessing } = options;
  const scrollRef = useRef<T | null>(null);
  const contentRef = useRef<U | null>(null);
  const [isAtBottom, setIsAtBottom] = useState(true);

  // 是否允许自动贴底（用户上滚时变为 false）
  const isAutoScrollEnabled = useRef(true);

  // 记录上一帧的 scrollTop，用于检测用户是否在主动滚动
  const lastScrollTop = useRef(0);

  // 检测用户是否正在主动滚动（用于减少渲染干扰）
  const isUserScrolling = useRef(false);
  const userScrollEndTimeout = useRef<number | null>(null);


  // 暴露给外部强制贴底（切换会话、新消息时调用）
  const scrollToBottom = useCallback(() => {
    const el = scrollRef.current;
    if (!el) return;

    // 使用 smooth 行为实现平滑滚动
    el.scrollTo({
      top: el.scrollHeight,
      behavior: 'smooth'
    });

    // 兜底：如果 smooth 滚动被中断，在下一帧强制贴底
    requestAnimationFrame(() => {
      if (scrollRef.current) {
        scrollRef.current.scrollTop = scrollRef.current.scrollHeight;
      }
    });

    isAutoScrollEnabled.current = true;
    setIsAtBottom(true);
  }, []);

  // 1. 依赖变化时同步贴底（新消息、切换会话等非流式场景）
  //    注意：处理中时不执行此逻辑，避免与 rAF 循环冲突
  useLayoutEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    if (isAutoScrollEnabled.current && !isProcessing) {
      el.scrollTop = el.scrollHeight;
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, deps);

  // 2. 核心修复：isProcessing 为 true 时，rAF 循环持续贴底
  //    流式输出的每一帧都会触发，不依赖 React 的依赖数组
  useEffect(() => {
    if (!isProcessing) return;

    let id: number;

    const tick = () => {
      const el = scrollRef.current;
      if (el) {
        const maxScroll = el.scrollHeight - el.clientHeight;
        const currentScrollTop = el.scrollTop;

        // 检测用户是否在主动向上滚动
        if (currentScrollTop < lastScrollTop.current - 5) {
          // 用户向上滚动，禁用自动滚动
          isAutoScrollEnabled.current = false;
          setIsAtBottom(false);
          isUserScrolling.current = true;

          // 清除之前的超时
          if (userScrollEndTimeout.current) {
            clearTimeout(userScrollEndTimeout.current);
          }

          // 设置超时，500ms 后认为用户停止滚动
          userScrollEndTimeout.current = window.setTimeout(() => {
            isUserScrolling.current = false;
          }, 500);
        }

        // 检测是否接近底部
        const threshold = 20;
        const isNearBottom = maxScroll - currentScrollTop <= threshold;

        if (isNearBottom && !isAutoScrollEnabled.current) {
          // 用户滚回底部，重新启用自动滚动
          isAutoScrollEnabled.current = true;
          setIsAtBottom(true);
          isUserScrolling.current = false;
        }

        // 用户滚动期间完全禁用自动滚动操作，避免干扰自然惯性
        if (isUserScrolling.current) {
          // 只更新状态，不执行任何滚动操作
          id = requestAnimationFrame(tick);
          return;
        }

        // 只有在启用自动滚动且不在底部时才滚动
        if (isAutoScrollEnabled.current && currentScrollTop < maxScroll - 1) {
          // 流式输出时使用 instant 滚动，避免 smooth 滚动在 rAF 循环中造成抖动
          el.scrollTop = maxScroll;
        }

        // 更新上一帧的 scrollTop
        lastScrollTop.current = currentScrollTop;
      }
      id = requestAnimationFrame(tick);
    };

    // 启动循环
    id = requestAnimationFrame(tick);

    return () => {
      cancelAnimationFrame(id);
      if (userScrollEndTimeout.current) {
        clearTimeout(userScrollEndTimeout.current);
      }
    };
  }, [isProcessing]);

  // 3. 监听用户手动滚动，决定是否解除自动贴底
  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;

    const handleScroll = () => {
      // 20px 阈值，兼容亚像素渲染和微小布局偏移
      const threshold = 20;
      const maxScroll = el.scrollHeight - el.clientHeight;
      const isNearBottom = maxScroll - el.scrollTop <= threshold;

      isAutoScrollEnabled.current = isNearBottom;
      setIsAtBottom(isNearBottom);
    };

    // passive: true 不阻塞滚动
    el.addEventListener('scroll', handleScroll, { passive: true });

    return () => el.removeEventListener('scroll', handleScroll);
  }, [scrollRef]);

  return { scrollRef, contentRef, scrollToBottom, isAtBottom };
}
