//! 内置 DNS 服务清单（设计方案 §4.3）
//!
//! 清单**编译进二进制、不进配置文件**（§7.1 第 1 点）：
//! 日后修正某个地址只需发版，用户无需迁移配置。
//! 地址来源为各服务商官网 / 官方文档，AdGuard 的 IPv6 采用官网公布的
//! `2a10:50c0::` 系列（第三方汇总站点写作 `2a00:5a60::…`，与官方不符，未采用）。

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DnsRegion {
    System,
    Domestic,
    Global,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DnsFeature {
    NoLog,
    AdBlock,
    MalwareBlock,
    Family,
    Encrypted,
    Fast,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct DnsPreset {
    pub id: String,
    pub name: String,
    pub region: DnsRegion,
    pub features: Vec<DnsFeature>,
    pub v4: Vec<String>,
    pub v6: Vec<String>,
    pub doh: Option<String>,
    pub dot: Option<String>,
    pub note: String,
    pub is_custom: bool,
    pub is_favorite: bool,
}

struct RawPreset {
    id: &'static str,
    name: &'static str,
    region: DnsRegion,
    v4: &'static [&'static str],
    v6: &'static [&'static str],
    doh: Option<&'static str>,
    dot: Option<&'static str>,
    features: &'static [DnsFeature],
    note: &'static str,
}

const fn f(x: &'static [DnsFeature]) -> &'static [DnsFeature] {
    x
}

const P: &[RawPreset] = &[
    // ---- 系统项 ----
    RawPreset {
        id: "system",
        name: "系统默认（DHCP）",
        region: DnsRegion::System,
        v4: &[],
        v6: &[],
        doh: None,
        dot: None,
        features: &[],
        note: "还原项：清空手动设置，回到自动获取",
    },
    // ---- 国内服务商 ----
    RawPreset {
        id: "ali",
        name: "阿里 AliDNS",
        region: DnsRegion::Domestic,
        v4: &["223.5.5.5", "223.6.6.6"],
        v6: &["2400:3200::1", "2400:3200:baba::1"],
        doh: Some("https://dns.alidns.com/dns-query"),
        dot: Some("dns.alidns.com"),
        features: f(&[DnsFeature::Encrypted, DnsFeature::Fast]),
        note: "国内覆盖最广，支持 DoT/DoH，ECS 调度",
    },
    RawPreset {
        id: "dnspod",
        name: "腾讯 DNSPod",
        region: DnsRegion::Domestic,
        v4: &["119.29.29.29", "182.254.116.116"],
        v6: &["2402:4e00::"],
        doh: Some("https://doh.pub/dns-query"),
        dot: Some("dot.pub"),
        features: f(&[DnsFeature::Encrypted, DnsFeature::MalwareBlock]),
        note: "腾讯系产品与游戏站点优化，防域名污染",
    },
    RawPreset {
        id: "baidu",
        name: "百度 DNS",
        region: DnsRegion::Domestic,
        v4: &["180.76.76.76"],
        v6: &["2400:da00::6666"],
        doh: None,
        dot: None,
        features: &[],
        note: "百度系站点优化",
    },
    RawPreset {
        id: "114",
        name: "114DNS（普通）",
        region: DnsRegion::Domestic,
        v4: &["114.114.114.114", "114.114.115.115"],
        v6: &[],
        doh: None,
        dot: None,
        features: &[],
        note: "老设备兼容性最好，不支持加密",
    },
    RawPreset {
        id: "114-safe",
        name: "114DNS（安全）",
        region: DnsRegion::Domestic,
        v4: &["114.114.114.119", "114.114.115.119"],
        v6: &[],
        doh: None,
        dot: None,
        features: f(&[DnsFeature::MalwareBlock]),
        note: "拦截钓鱼病毒木马",
    },
    RawPreset {
        id: "114-family",
        name: "114DNS（家庭）",
        region: DnsRegion::Domestic,
        v4: &["114.114.114.110", "114.114.115.110"],
        v6: &[],
        doh: None,
        dot: None,
        features: f(&[DnsFeature::Family]),
        note: "拦截成人内容",
    },
    RawPreset {
        id: "360-ct",
        name: "360 安全 DNS（电信/移动）",
        region: DnsRegion::Domestic,
        v4: &["101.226.4.6", "218.30.118.6"],
        v6: &[],
        doh: None,
        dot: None,
        features: f(&[DnsFeature::MalwareBlock]),
        note: "按运营商分流，需选对应线路",
    },
    RawPreset {
        id: "360-cu",
        name: "360 安全 DNS（联通）",
        region: DnsRegion::Domestic,
        v4: &["123.125.81.6", "140.207.198.6"],
        v6: &[],
        doh: None,
        dot: None,
        features: f(&[DnsFeature::MalwareBlock]),
        note: "按运营商分流，需选对应线路",
    },
    RawPreset {
        id: "cnnic",
        name: "CNNIC SDNS",
        region: DnsRegion::Domestic,
        v4: &["1.2.4.8", "210.2.4.8"],
        v6: &["2001:dc7:1000::1"],
        doh: None,
        dot: None,
        features: &[],
        note: "国家互联网信息中心，无广告",
    },
    RawPreset {
        id: "onedns-block",
        name: "OneDNS（拦截版）",
        region: DnsRegion::Domestic,
        v4: &["117.50.11.11", "52.80.66.66"],
        v6: &["2400:7fc0:849e:200::4", "2404:c2c0:85d8:901::4"],
        doh: None,
        dot: None,
        features: f(&[DnsFeature::AdBlock, DnsFeature::MalwareBlock]),
        note: "微步在线，过滤广告与恶意站点",
    },
    RawPreset {
        id: "onedns-pure",
        name: "OneDNS（纯净版）",
        region: DnsRegion::Domestic,
        v4: &["117.50.10.10", "52.80.52.52"],
        v6: &["2400:7fc0:849e:200::8", "2404:c2c0:85d8:901::8"],
        doh: None,
        dot: None,
        features: &[],
        note: "不过滤，直接返回真实结果",
    },
    RawPreset {
        id: "bytedance",
        name: "字节跳动公共 DNS",
        region: DnsRegion::Domestic,
        v4: &["180.184.1.1", "180.184.2.2"],
        v6: &[],
        doh: None,
        dot: None,
        features: f(&[DnsFeature::Fast]),
        note: "抖音/头条系站点优化",
    },
    // ---- 国外服务商 ----
    RawPreset {
        id: "cloudflare",
        name: "Cloudflare",
        region: DnsRegion::Global,
        v4: &["1.1.1.1", "1.0.0.1"],
        v6: &["2606:4700:4700::1111", "2606:4700:4700::1001"],
        doh: Some("https://cloudflare-dns.com/dns-query"),
        dot: Some("cloudflare-dns.com"),
        features: f(&[DnsFeature::NoLog, DnsFeature::Fast, DnsFeature::Encrypted]),
        note: "独立基准测试中最快，隐私优先，24 小时清除日志",
    },
    RawPreset {
        id: "cloudflare-family",
        name: "Cloudflare（家庭版）",
        region: DnsRegion::Global,
        v4: &["1.1.1.3", "1.0.0.3"],
        v6: &["2606:4700:4700::1113", "2606:4700:4700::1003"],
        doh: Some("https://family.cloudflare-dns.com/dns-query"),
        dot: None,
        features: f(&[DnsFeature::Family]),
        note: "拦截成人内容",
    },
    RawPreset {
        id: "google",
        name: "Google",
        region: DnsRegion::Global,
        v4: &["8.8.8.8", "8.8.4.4"],
        v6: &["2001:4860:4860::8888", "2001:4860:4860::8844"],
        doh: Some("https://dns.google/dns-query"),
        dot: Some("dns.google"),
        features: f(&[DnsFeature::Encrypted]),
        note: "全球最大，24–48 小时匿名日志",
    },
    RawPreset {
        id: "quad9",
        name: "Quad9",
        region: DnsRegion::Global,
        v4: &["9.9.9.9", "149.112.112.112"],
        v6: &["2620:fe::fe", "2620:fe::9"],
        doh: Some("https://dns.quad9.net/dns-query"),
        dot: Some("dns.quad9.net"),
        features: f(&[DnsFeature::MalwareBlock, DnsFeature::Encrypted]),
        note: "默认拦截恶意域名，瑞士基金会，DNSSEC 强制校验",
    },
    RawPreset {
        id: "quad9-nofilter",
        name: "Quad9（不拦截）",
        region: DnsRegion::Global,
        v4: &["9.9.9.10"],
        v6: &["2620:fe::10"],
        doh: Some("https://dns10.quad9.net/dns-query"),
        dot: None,
        features: f(&[DnsFeature::Encrypted]),
        note: "只校验 DNSSEC，不过滤，适合怕误杀",
    },
    RawPreset {
        id: "opendns",
        name: "OpenDNS（Cisco）",
        region: DnsRegion::Global,
        v4: &["208.67.222.222", "208.67.220.220"],
        v6: &["2620:119:35::35", "2620:119:53::53"],
        doh: Some("https://doh.opendns.com/dns-query"),
        dot: None,
        features: f(&[DnsFeature::Family]),
        note: "家长控制与域名过滤",
    },
    RawPreset {
        id: "adguard",
        name: "AdGuard（默认）",
        region: DnsRegion::Global,
        v4: &["94.140.14.14", "94.140.15.15"],
        v6: &["2a10:50c0::ad1:ff", "2a10:50c0::ad2:ff"],
        doh: Some("https://dns.adguard-dns.com/dns-query"),
        dot: Some("dns.adguard-dns.com"),
        features: f(&[DnsFeature::AdBlock, DnsFeature::NoLog, DnsFeature::Encrypted]),
        note: "拦截广告与跟踪器，全球 100+ 节点",
    },
    RawPreset {
        id: "adguard-family",
        name: "AdGuard（家庭）",
        region: DnsRegion::Global,
        v4: &["94.140.14.15", "94.140.15.16"],
        v6: &["2a10:50c0::bad1:ff", "2a10:50c0::bad2:ff"],
        doh: Some("https://family.adguard-dns.com/dns-query"),
        dot: None,
        features: f(&[DnsFeature::Family, DnsFeature::AdBlock]),
        note: "追加成人内容拦截 + 安全搜索",
    },
    RawPreset {
        id: "adguard-unfiltered",
        name: "AdGuard（无过滤）",
        region: DnsRegion::Global,
        v4: &["94.140.14.140", "94.140.14.141"],
        v6: &["2a10:50c0::1:ff", "2a10:50c0::2:ff"],
        doh: Some("https://unfiltered.adguard-dns.com/dns-query"),
        dot: None,
        features: f(&[DnsFeature::NoLog]),
        note: "只加速，不过滤",
    },
    RawPreset {
        id: "cleanbrowsing-security",
        name: "CleanBrowsing（安全）",
        region: DnsRegion::Global,
        v4: &["185.228.168.9", "185.228.169.9"],
        v6: &["2a0d:2a00:1::2", "2a0d:2a00:2::2"],
        doh: Some("https://doh.cleanbrowsing.org/doh/security-filter/"),
        dot: None,
        features: f(&[DnsFeature::MalwareBlock]),
        note: "拦截钓鱼、垃圾邮件、恶意软件",
    },
    RawPreset {
        id: "cleanbrowsing-family",
        name: "CleanBrowsing（家庭）",
        region: DnsRegion::Global,
        v4: &["185.228.168.168", "185.228.169.168"],
        v6: &["2a0d:2a00:1::1", "2a0d:2a00:2::1"],
        doh: Some("https://doh.cleanbrowsing.org/doh/family-filter/"),
        dot: None,
        features: f(&[DnsFeature::Family]),
        note: "追加成人内容拦截",
    },
    RawPreset {
        id: "controld",
        name: "Control D",
        region: DnsRegion::Global,
        v4: &["76.76.2.0", "76.76.10.0"],
        v6: &["2606:1a40::", "2606:1a40:1::"],
        doh: Some("https://freedns.controld.com/p0"),
        dot: None,
        features: f(&[DnsFeature::Encrypted]),
        note: "多配置文件可选，免费额度够用",
    },
    RawPreset {
        id: "nextdns",
        name: "NextDNS",
        region: DnsRegion::Global,
        v4: &["45.90.28.0", "45.90.30.0"],
        v6: &["2a07:a8c0::", "2a07:a8c1::"],
        doh: Some("https://dns.nextdns.io"),
        dot: None,
        features: f(&[DnsFeature::AdBlock, DnsFeature::Encrypted]),
        note: "可自定义屏蔽清单与统计，免费 30 万次/月",
    },
    RawPreset {
        id: "mullvad",
        name: "Mullvad",
        region: DnsRegion::Global,
        v4: &["194.242.2.2", "193.19.108.2"],
        v6: &["2a07:e340::2"],
        doh: Some("https://dns.mullvad.net/dns-query"),
        dot: None,
        features: f(&[DnsFeature::NoLog, DnsFeature::Encrypted]),
        note: "零日志，含 DoH/DoT",
    },
    RawPreset {
        id: "yandex",
        name: "Yandex",
        region: DnsRegion::Global,
        v4: &["77.88.8.8", "77.88.8.1"],
        v6: &["2a02:6b8::feed:0ff", "2a02:6b8:0:1::feed:0ff"],
        doh: None,
        dot: None,
        features: &[],
        note: "俄罗斯服务，国内延迟高",
    },
    RawPreset {
        id: "dnswatch",
        name: "DNS.WATCH",
        region: DnsRegion::Global,
        v4: &["84.200.69.80", "84.200.70.40"],
        v6: &[
            "2001:1608:10:25::1c04:b12f",
            "2001:1608:10:25::9249:d69b",
        ],
        doh: None,
        dot: None,
        features: f(&[DnsFeature::NoLog]),
        note: "无过滤、无日志、不记录 IP",
    },
    RawPreset {
        id: "dnssb",
        name: "DNS.SB",
        region: DnsRegion::Global,
        v4: &["185.222.222.222", "45.11.45.11"],
        v6: &["2a09::", "2a11::"],
        doh: Some("https://doh.dns.sb/dns-query"),
        dot: None,
        features: f(&[DnsFeature::NoLog]),
        note: "欧洲，非营利",
    },
    RawPreset {
        id: "comodo",
        name: "Comodo Secure",
        region: DnsRegion::Global,
        v4: &["8.26.56.26", "8.20.247.20"],
        v6: &[],
        doh: None,
        dot: None,
        features: f(&[DnsFeature::MalwareBlock]),
        note: "恶意软件拦截",
    },
    RawPreset {
        id: "ultradns",
        name: "UltraDNS（Verisign）",
        region: DnsRegion::Global,
        v4: &["156.154.70.1", "156.154.71.1"],
        v6: &["2610:a1:1018::1", "2610:a1:1019::1"],
        doh: None,
        dot: None,
        features: &[],
        note: "域名安全，家庭/企业过滤可选",
    },
    RawPreset {
        id: "he",
        name: "Hurricane Electric",
        region: DnsRegion::Global,
        v4: &["74.82.42.42"],
        v6: &["2001:470:20::2"],
        doh: None,
        dot: None,
        features: &[],
        note: "老牌 Anycast",
    },
];

/// 内置清单（不含用户自定义项）
pub fn builtin() -> Vec<DnsPreset> {
    P.iter()
        .map(|r| DnsPreset {
            id: r.id.to_string(),
            name: r.name.to_string(),
            region: r.region,
            features: r.features.to_vec(),
            v4: r.v4.iter().map(|s| s.to_string()).collect(),
            v6: r.v6.iter().map(|s| s.to_string()).collect(),
            doh: r.doh.map(|s| s.to_string()),
            dot: r.dot.map(|s| s.to_string()),
            note: r.note.to_string(),
            is_custom: false,
            is_favorite: false,
        })
        .collect()
}

/// 名称筛选：按名称 / 备注 / 地址子串匹配（§8 dns_presets 的 query 参数）
pub fn search(query: &str, custom: &[DnsPreset]) -> Vec<DnsPreset> {
    let q = query.trim().to_lowercase();
    let mut all: Vec<DnsPreset> = builtin();
    all.extend(custom.iter().cloned());
    if q.is_empty() {
        return all;
    }
    all.into_iter()
        .filter(|p| {
            p.name.to_lowercase().contains(&q)
                || p.note.to_lowercase().contains(&q)
                || p.id.to_lowercase().contains(&q)
                || p.v4.iter().any(|a| a.contains(&q))
                || p.v6.iter().any(|a| a.contains(&q))
        })
        .collect()
}

#[derive(Serialize, Clone, Debug)]
pub struct DohReference {
    pub name: String,
    pub doh: String,
    pub dot: String,
}

/// DoH / DoT 端点参考表（§4.4：v1 不做 DoH，但必须提供只读提示区）
pub fn doh_reference() -> Vec<DohReference> {
    builtin()
        .into_iter()
        .filter(|p| p.doh.is_some())
        .map(|p| DohReference {
            name: p.name.clone(),
            doh: p.doh.unwrap_or_default(),
            dot: p.dot.unwrap_or_default(),
        })
        .collect()
}

/// 内置关键域名清单（§10.1 敏感域名保护：银行 / 支付 / 政务 / 邮箱 / 常见 CA）
pub fn sensitive_domains() -> &'static [&'static str] {
    &[
        // 银行与支付
        "icbc.com.cn",
        "ccb.com",
        "abchina.com",
        "boc.cn",
        "bankcomm.com",
        "cmbchina.com",
        "cebbank.com",
        "psbc.com",
        "spdb.com.cn",
        "citicbank.com",
        "alipay.com",
        "paypal.com",
        "visa.com",
        "mastercard.com",
        "unionpay.com",
        "unionpayintl.com",
        // 政务与公共服务
        "gov.cn",
        "12306.cn",
        "chinatax.gov.cn",
        "mps.gov.cn",
        "beian.miit.gov.cn",
        "creditchina.gov.cn",
        // 邮箱与云
        "qq.com",
        "163.com",
        "126.com",
        "outlook.com",
        "office.com",
        "live.com",
        "gmail.com",
        "aliyun.com",
        "aliyuncs.com",
        "tencentcloudapi.com",
        "myqcloud.com",
        // 账号与身份
        "weixin.qq.com",
        "login.gov.cn",
        "apple.com",
        "icloud.com",
        "microsoft.com",
        "windowsupdate.com",
        // 常见 CA / 证书校验
        "digicert.com",
        "letsencrypt.org",
        "ocsp.digicert.com",
        "crl.microsoft.com",
        "globalsign.com",
        "sectigo.com",
    ]
}

/// 判断某域名是否命中敏感清单（含子域）
pub fn is_sensitive(hostname: &str) -> bool {
    let h = hostname.trim().to_ascii_lowercase();
    sensitive_domains()
        .iter()
        .any(|d| h == *d || h.ends_with(&format!(".{d}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_addresses_parse() {
        for p in builtin() {
            for a in p.v4.iter() {
                assert!(
                    a.parse::<std::net::Ipv4Addr>().is_ok(),
                    "{} 的 IPv4 地址非法：{a}",
                    p.name
                );
            }
            for a in p.v6.iter() {
                assert!(
                    a.parse::<std::net::Ipv6Addr>().is_ok(),
                    "{} 的 IPv6 地址非法：{a}",
                    p.name
                );
            }
        }
    }

    #[test]
    fn preset_ids_unique() {
        let list = builtin();
        let mut ids: Vec<&str> = list.iter().map(|p| p.id.as_str()).collect();
        ids.sort_unstable();
        let n = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), n, "存在重复的预设 id");
        assert!(list.len() >= 30, "内置清单条目不足：{}", list.len());
    }

    #[test]
    fn sensitive_match() {
        assert!(is_sensitive("icbc.com.cn"));
        assert!(is_sensitive("mail.163.com"));
        assert!(!is_sensitive("example.com"));
    }
}
