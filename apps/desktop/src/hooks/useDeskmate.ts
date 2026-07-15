// deskmate 前端状态核心: 订阅引擎事件, 聚合节点/传输/文本/请求状态

import { useCallback, useEffect, useMemo, useReducer, useRef, useState } from "react";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { api } from "../api";
import { EVENTS } from "../events";
import {
  avatarBlobUrl,
  avatarHashOf,
  type OfferDto,
  type PeerDto,
  type SelfInfoDto,
  type TextMsg,
  type TransferEventDto,
  type TransferItem,
} from "../types";

/** 速度平滑采样: 按 transferId 记录上一次进度(生命周期随组件, 见 makeTransferReducer) */
type SpeedSamples = Map<string, { done: number; at: number; file: string }>;

type TransferAction =
  | {
      type: "begin";
      transferId: string;
      direction: "send" | "recv";
      peerName: string;
      peerFingerprint: string;
    }
  | { type: "event"; event: Exclude<TransferEventDto, { kind: "textReceived" }>; at: number };

/** 构造传输事件聚合 reducer: 把逐文件的引擎事件折叠成面板条目
 *
 * 速度采样表经参数注入(而非模块级共享), 随组件实例创建与回收,
 * 避免卸载后残留未终态任务的采样数据。 */
function makeTransferReducer(speedSamples: SpeedSamples) {
  return function transferReducer(
    state: Record<string, TransferItem>,
    action: TransferAction,
  ): Record<string, TransferItem> {
  if (action.type === "begin") {
    const { transferId, direction, peerName, peerFingerprint } = action;
    return {
      ...state,
      [transferId]: {
        transferId,
        direction,
        peerName,
        peerFingerprint,
        status: "active",
        currentFile: "等待对方响应…",
        done: 0,
        size: 0,
        filesDone: 0,
        speed: 0,
        startedAt: Date.now(),
      },
    };
  }

  const ev = action.event;
  // 未知任务兜底(理论上 begin 先到): 按接收方向补建
  const prev: TransferItem = state[ev.transferId] ?? {
    transferId: ev.transferId,
    direction: "recv",
    peerName: "对方设备",
    peerFingerprint: "",
    status: "active",
    currentFile: "",
    done: 0,
    size: 0,
    filesDone: 0,
    speed: 0,
    startedAt: action.at,
  };

  let next: TransferItem = prev;
  switch (ev.kind) {
    case "progress": {
      // 速度: 同一文件内的增量 / 时间差, 指数平滑
      let speed = prev.speed;
      const sample = speedSamples.get(ev.transferId);
      if (sample && sample.file === ev.relPath && ev.done > sample.done) {
        const dt = (action.at - sample.at) / 1000;
        if (dt > 0) {
          const inst = (ev.done - sample.done) / dt;
          speed = speed === 0 ? inst : speed * 0.7 + inst * 0.3;
        }
      }
      speedSamples.set(ev.transferId, { done: ev.done, at: action.at, file: ev.relPath });
      next = {
        ...prev,
        status: prev.status === "paused" ? "paused" : "active",
        currentFile: ev.relPath,
        done: ev.done,
        size: ev.size,
        speed,
      };
      break;
    }
    case "fileCompleted":
      next = { ...prev, filesDone: prev.filesDone + 1, lastPath: ev.path };
      break;
    case "completed":
      speedSamples.delete(ev.transferId);
      next = { ...prev, status: "completed", speed: 0, done: prev.size };
      break;
    case "cancelled":
      speedSamples.delete(ev.transferId);
      next = { ...prev, status: "cancelled", speed: 0 };
      break;
    case "interrupted":
      speedSamples.delete(ev.transferId);
      next = { ...prev, status: "interrupted", speed: 0, reason: ev.reason };
      break;
    case "rejected":
      speedSamples.delete(ev.transferId);
      next = {
        ...prev,
        status: "rejected",
        speed: 0,
        reason: ev.reason ?? "对方拒绝",
        pinRequired: ev.pinRequired,
      };
      break;
  }
  return { ...state, [ev.transferId]: next };
  };
}

/** 应用状态与操作的统一入口 */
export function useDeskmate() {
  const [self, setSelf] = useState<SelfInfoDto | null>(null);
  const [peers, setPeers] = useState<Record<string, PeerDto>>({});
  const [offers, setOffers] = useState<OfferDto[]>([]);
  const [texts, setTexts] = useState<TextMsg[]>([]);
  // 图片头像: hash → Blob URL(节点与本机共用)
  const [avatarSrcs, setAvatarSrcs] = useState<Record<string, string>>({});
  // 速度采样与 reducer 绑定同一组件实例(useMemo 保 reducer 引用稳定)
  const speedSamples = useRef<SpeedSamples>(new Map());
  const reducer = useMemo(() => makeTransferReducer(speedSamples.current), []);
  const [transfers, dispatch] = useReducer(reducer, {});
  // offer → 接受后需要知道 peerName 建立接收条目
  const offersRef = useRef<OfferDto[]>([]);
  offersRef.current = offers;
  // 终态上报历史时需要事件到达前的聚合状态(peerName/进度), ref 避免闭包过期
  const transfersRef = useRef(transfers);
  transfersRef.current = transfers;
  // 加载中/已加载的头像哈希(防重复请求; ref 避免闭包读到过期 state)
  const avatarSeen = useRef(new Set<string>());
  // 卸载时回收全部 Blob URL(会话内换头像的旧 URL 体量很小, 容忍到卸载统一回收)
  const avatarSrcsRef = useRef(avatarSrcs);
  avatarSrcsRef.current = avatarSrcs;
  useEffect(
    () => () => {
      for (const url of Object.values(avatarSrcsRef.current)) URL.revokeObjectURL(url);
    },
    [],
  );
  // 对端 PIN 会话缓存(fingerprint → pin), 发送时自动附带, 不持久化
  const pinCache = useRef(new Map<string, string>());

  /** 取会话缓存的对端 PIN(useCallback 保引用稳定, 供 memo 子组件使用) */
  const getPin = useCallback((fingerprint: string) => pinCache.current.get(fingerprint), []);
  /** 记住验证通过的对端 PIN(本次运行内有效) */
  const rememberPin = useCallback((fingerprint: string, pin: string) => {
    pinCache.current.set(fingerprint, pin);
  }, []);

  /** 按哈希加载头像字节转 Blob URL; self 为 true 时读本机自定义头像文件 */
  const loadAvatar = (hash: string | null, isSelf = false) => {
    if (!hash || avatarSeen.current.has(hash)) return;
    avatarSeen.current.add(hash);
    api
      .getAvatarImage(isSelf ? undefined : hash)
      .then((bytes) => {
        if (bytes && bytes.length > 0) {
          setAvatarSrcs((prev) => ({ ...prev, [hash]: avatarBlobUrl(bytes) }));
        } else {
          // 缓存未命中: 允许 avatar-ready 到达后重试
          avatarSeen.current.delete(hash);
        }
      })
      .catch(() => avatarSeen.current.delete(hash));
  };

  useEffect(() => {
    let alive = true;
    const unsubs: UnlistenFn[] = [];
    const add = (p: Promise<UnlistenFn>) =>
      p.then((u) => {
        // StrictMode 下 effect 会双跑, 迟到的订阅立即退订
        if (alive) unsubs.push(u);
        else u();
      });

    // 纯浏览器预览(无 Tauri 运行时)下 listen 同步抛错, 不能崩掉整树
    if (!("__TAURI_INTERNALS__" in window)) {
      console.warn("非 Tauri 环境: 跳过引擎事件订阅(仅静态预览)");
      return;
    }

    add(
      listen<PeerDto>(EVENTS.PEER_UP, (e) => {
        loadAvatar(avatarHashOf(e.payload.avatar));
        setPeers((prev) => ({ ...prev, [e.payload.fingerprint]: e.payload }));
      }),
    );
    // 后台拉取完成: 重新读缓存
    add(
      listen<{ fingerprint: string; hash: string }>(EVENTS.AVATAR_READY, (e) =>
        loadAvatar(e.payload.hash),
      ),
    );
    // 白名单自动接收: 无确认弹窗, 直接建立接收进度条目
    add(
      listen<{ transferId: string; peerName: string }>(EVENTS.TRANSFER_AUTOSTART, (e) =>
        dispatch({
          type: "begin",
          transferId: e.payload.transferId,
          direction: "recv",
          peerName: e.payload.peerName,
          peerFingerprint: "",
        }),
      ),
    );
    add(
      listen<string>(EVENTS.PEER_DOWN, (e) =>
        setPeers((prev) => {
          const next = { ...prev };
          delete next[e.payload];
          return next;
        }),
      ),
    );
    add(listen<OfferDto>(EVENTS.TRANSFER_OFFER, (e) => setOffers((prev) => [...prev, e.payload])));
    add(
      listen<TransferEventDto>(EVENTS.TRANSFER_EVENT, (e) => {
        const ev = e.payload;
        if (ev.kind === "textReceived") {
          setTexts((prev) =>
            [
              {
                id: `${Date.now()}-${Math.random().toString(36).slice(2, 8)}`,
                direction: "in" as const,
                peerName: ev.fromName,
                text: ev.text,
                at: Date.now(),
              },
              ...prev,
            ].slice(0, 50),
          );
        } else {
          dispatch({ type: "event", event: ev, at: Date.now() });
          // 终态写入历史(此刻 ref 是事件前的聚合状态, filesDone/done 已随先前事件累计)
          if (
            ev.kind === "completed" ||
            ev.kind === "cancelled" ||
            ev.kind === "interrupted" ||
            ev.kind === "rejected"
          ) {
            const prev = transfersRef.current[ev.transferId];
            api
              .appendHistory({
                transferId: ev.transferId,
                direction: prev?.direction ?? "recv",
                peerName: prev?.peerName ?? "对方设备",
                status: ev.kind,
                filesDone: prev?.filesDone ?? 0,
                bytes: prev?.done ?? 0,
                at: Date.now(),
                lastPath: prev?.lastPath ?? null,
              })
              .catch(console.error);
          }
        }
      }),
    );

    // 初始快照
    api
      .getSelfInfo()
      .then((info) => {
        if (!alive) return;
        setSelf(info);
        loadAvatar(avatarHashOf(info.avatar), true);
      })
      .catch(console.error);
    api
      .listPeers()
      .then((list) => {
        if (!alive) return;
        setPeers(Object.fromEntries(list.map((p) => [p.fingerprint, p])));
        list.forEach((p) => loadAvatar(avatarHashOf(p.avatar)));
      })
      .catch(console.error);

    return () => {
      alive = false;
      unsubs.forEach((u) => u());
    };
  }, []);

  /** 发送文件到节点(paths 为绝对路径; useCallback 与其余操作保持引用稳定) */
  const sendFiles = useCallback(
    async (peer: PeerDto, paths: string[]) => {
      const transferId = await api.sendFiles(peer.fingerprint, paths, getPin(peer.fingerprint));
      dispatch({
        type: "begin",
        transferId,
        direction: "send",
        peerName: peer.name,
        peerFingerprint: peer.fingerprint,
      });
    },
    [getPin],
  );

  /** 发送剪贴板截图到节点(先经 raw 通道暂存字节, 再走文件传输链;
   * useCallback 保引用稳定, 供 memo 的 TransferPanel 使用) */
  const sendClipboardImage = useCallback(
    async (peer: PeerDto, fileName: string, bytes: Uint8Array) => {
      const staged = await api.stageClipboardImage(bytes);
      const transferId = await api.sendClipboardImage(
        peer.fingerprint,
        fileName,
        staged,
        getPin(peer.fingerprint),
      );
      dispatch({
        type: "begin",
        transferId,
        direction: "send",
        peerName: peer.name,
        peerFingerprint: peer.fingerprint,
      });
    },
    [getPin],
  );

  /** 应答接收请求; overwrite 为本次的同名冲突决策 */
  const respondOffer = useCallback(
    async (offer: OfferDto, accept: boolean, opts?: { saveDir?: string; overwrite?: boolean }) => {
      await api.respondOffer(offer.offerId, accept, opts?.saveDir, opts?.overwrite ?? false);
      setOffers((prev) => prev.filter((o) => o.offerId !== offer.offerId));
      if (accept) {
        dispatch({
          type: "begin",
          transferId: offer.transferId,
          direction: "recv",
          peerName: offer.peerName,
          peerFingerprint: offer.peerFingerprint,
        });
      }
    },
    [],
  );

  /** 记录一条发出的文本进消息流(聊天输入框发送成功后调用) */
  const addSentText = useCallback((peerName: string, text: string) => {
    setTexts((prev) =>
      [
        {
          id: `${Date.now()}-${Math.random().toString(36).slice(2, 8)}`,
          direction: "out" as const,
          peerName,
          text,
          at: Date.now(),
        },
        ...prev,
      ].slice(0, 50),
    );
  }, []);

  /** 删除一条文字消息(仅内存态) */
  const removeText = useCallback((id: string) => {
    setTexts((prev) => prev.filter((m) => m.id !== id));
  }, []);

  /** 清空全部文字消息(仅内存态) */
  const clearTexts = useCallback(() => setTexts([]), []);

  /** 重新拉取本机信息(设置保存后昵称/头像即时刷新, 含新头像 Blob 加载) */
  const refreshSelf = () => {
    api
      .getSelfInfo()
      .then((info) => {
        setSelf(info);
        loadAvatar(avatarHashOf(info.avatar), true);
      })
      .catch(console.error);
  };

  return {
    self,
    peers,
    offers,
    texts,
    transfers,
    avatarSrcs,
    sendFiles,
    sendClipboardImage,
    respondOffer,
    getPin,
    rememberPin,
    addSentText,
    removeText,
    clearTexts,
    refreshSelf,
    dispatch,
  };
}
