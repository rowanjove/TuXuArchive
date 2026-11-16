use crate::model::NamingRule;

pub struct ExampleTagSpec {
    pub id: &'static str,
    pub category: &'static str,
    pub name: &'static str,
    pub short_label: &'static str,
    pub aliases: &'static [&'static str],
    pub sort_order: i64,
}

pub struct ExamplePresetSpec {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub scene_type: &'static str,
    pub group_names: &'static [&'static str],
    pub tag_names: &'static [&'static str],
    pub default_pattern: &'static str,
}

pub const EXAMPLE_TAGS: &[ExampleTagSpec] = &[
    ExampleTagSpec {
        id: "tag-sample-cover",
        category: "通用示例 · 摄影相册",
        name: "封面精选",
        short_label: "封面",
        aliases: &["cover", "精选", "主图"],
        sort_order: 10,
    },
    ExampleTagSpec {
        id: "tag-sample-landscape",
        category: "通用示例 · 摄影相册",
        name: "风景远景",
        short_label: "风景",
        aliases: &["scenery", "全景", "landscape"],
        sort_order: 20,
    },
    ExampleTagSpec {
        id: "tag-sample-portrait",
        category: "通用示例 · 摄影相册",
        name: "人物合影",
        short_label: "合影",
        aliases: &["portrait", "人像", "团体照"],
        sort_order: 30,
    },
    ExampleTagSpec {
        id: "tag-sample-food",
        category: "通用示例 · 摄影相册",
        name: "美食打卡",
        short_label: "美食",
        aliases: &["food", "餐饮"],
        sort_order: 40,
    },
    ExampleTagSpec {
        id: "tag-sample-front",
        category: "通用示例 · 商品素材",
        name: "正面主图",
        short_label: "正面",
        aliases: &["front", "白底图", "主图"],
        sort_order: 50,
    },
    ExampleTagSpec {
        id: "tag-sample-detail",
        category: "通用示例 · 商品素材",
        name: "细节特写",
        short_label: "细节",
        aliases: &["detail", "微距", "局部"],
        sort_order: 60,
    },
    ExampleTagSpec {
        id: "tag-sample-scene",
        category: "通用示例 · 商品素材",
        name: "场景展示",
        short_label: "场景",
        aliases: &["scene", "实拍"],
        sort_order: 70,
    },
    ExampleTagSpec {
        id: "tag-sample-package",
        category: "通用示例 · 商品素材",
        name: "包装清单",
        short_label: "包装",
        aliases: &["package", "配件", "装箱"],
        sort_order: 80,
    },
    ExampleTagSpec {
        id: "tag-sample-doc",
        category: "通用示例 · 文档票据",
        name: "合同正文",
        short_label: "正文",
        aliases: &["contract", "文件", "扫描件"],
        sort_order: 90,
    },
    ExampleTagSpec {
        id: "tag-sample-invoice",
        category: "通用示例 · 文档票据",
        name: "发票凭证",
        short_label: "发票",
        aliases: &["invoice", "报销", "收据"],
        sort_order: 100,
    },
    ExampleTagSpec {
        id: "tag-sample-id",
        category: "通用示例 · 文档票据",
        name: "证件材料",
        short_label: "证件",
        aliases: &["idcard", "执照", "证明"],
        sort_order: 110,
    },
    ExampleTagSpec {
        id: "tag-sample-ref",
        category: "通用示例 · 设计归档",
        name: "参考素材",
        short_label: "参考",
        aliases: &["reference", "灵感"],
        sort_order: 120,
    },
    ExampleTagSpec {
        id: "tag-sample-final",
        category: "通用示例 · 设计归档",
        name: "成片交付",
        short_label: "成片",
        aliases: &["final", "导出", "定稿"],
        sort_order: 130,
    },
    ExampleTagSpec {
        id: "tag-sample-backup",
        category: "通用示例 · 设计归档",
        name: "过程原图",
        short_label: "原图",
        aliases: &["raw", "过程", "备份"],
        sort_order: 140,
    },
];

pub const EXAMPLE_PRESETS: &[ExamplePresetSpec] = &[
    ExamplePresetSpec {
        id: "preset-example-travel",
        name: "示例：旅行相册整理",
        description: "通用示例方案：按旅行常见主题创建分类目录，支持每类多张连拍编号",
        scene_type: "摄影相册",
        group_names: &["沿途风景", "人物合影", "美食打卡", "精选封面"],
        tag_names: &["封面精选", "风景远景", "人物合影", "美食打卡"],
        default_pattern: "{date}_{group}_{seq}",
    },
    ExamplePresetSpec {
        id: "preset-example-ecommerce",
        name: "示例：商品拍摄归档",
        description: "通用示例方案：适用于产品图库整理（主图/细节/场景/包装多图归档）",
        scene_type: "商品素材",
        group_names: &["01_正面主图", "02_细节特写", "03_场景展示", "04_包装清单"],
        tag_names: &["正面主图", "细节特写", "场景展示", "包装清单"],
        default_pattern: "{group}_{seq}",
    },
    ExamplePresetSpec {
        id: "preset-example-docs",
        name: "示例：文档与票据归档",
        description: "通用示例方案：适用于扫描件、发票收据与证件照分目录留存",
        scene_type: "文档票据",
        group_names: &["合同文件", "发票凭证", "证件材料"],
        tag_names: &["合同正文", "发票凭证", "证件材料"],
        default_pattern: "{date}_{group}_{seq}",
    },
    ExamplePresetSpec {
        id: "preset-example-design",
        name: "示例：日常素材与成片",
        description: "通用示例方案：将成片交付、过程原图与参考素材分开归档",
        scene_type: "设计归档",
        group_names: &["成片交付", "过程原图", "参考素材"],
        tag_names: &["成片交付", "过程原图", "参考素材"],
        default_pattern: "{prefix}{group}_{seq}",
    },
];

pub fn default_naming_for_pattern(pattern: &str) -> NamingRule {
    NamingRule {
        preset: "custom".into(),
        pattern: pattern.into(),
        ..NamingRule::default()
    }
}

pub fn category_of(name: &str) -> &'static str {
    EXAMPLE_TAGS
        .iter()
        .find(|entry| entry.name == name)
        .map(|entry| entry.category)
        .unwrap_or("自定义标签")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn example_presets_reference_valid_tags_with_no_duplicates() {
        let mut names = BTreeSet::new();
        for tag in EXAMPLE_TAGS {
            assert!(names.insert(tag.name), "duplicate tag: {}", tag.name);
        }
        let mut preset_ids = BTreeSet::new();
        for preset in EXAMPLE_PRESETS {
            assert!(preset_ids.insert(preset.id), "duplicate preset id");
            for tn in preset.tag_names {
                assert!(names.contains(tn), "missing tag {tn}");
            }
        }
    }
}
