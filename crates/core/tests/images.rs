//! 节点图片：保存、格式校验、封面、说明、删除、随节点删除；导入导出包往返（含复制模式与新建世界）。

mod common;

use base64::Engine;
use chronicle_core::bundle::{self, ConflictStrategy};
use chronicle_core::images;
use chronicle_core::*;
use common::*;

fn png() -> Vec<u8> {
    base64::engine::general_purpose::STANDARD
        .decode("iVBORw0KGgoAAAANSUhEUgAAABAAAAAQCAIAAACQkWg2AAAAF0lEQVR4nGN4ICBAEmIY1TCqQWDYagAA6ggAEK2bvLwAAAAASUVORK5CYII=")
        .unwrap()
}

fn jpeg_like() -> Vec<u8> {
    let mut v = vec![0xff, 0xd8, 0xff, 0xe0];
    v.extend(std::iter::repeat_n(7u8, 200));
    v
}

#[test]
fn add_list_cover_caption_delete() {
    let mut e = env();
    let n = mk(&mut e.world, NewNode::new(NodeKind::Character, "老板娘梅"));
    let c = e.world.conn();
    let a = images::add_image(c, &n.id, &png(), 16, 16, "  立绘  ").unwrap();
    assert_eq!((a.mime.as_str(), a.caption.as_str(), a.width), ("image/png", "立绘", 16));
    let b = images::add_image(c, &n.id, &jpeg_like(), 800, 600, "酒馆里").unwrap();
    assert_eq!(b.mime, "image/jpeg", "格式按文件头判断");
    assert_eq!(images::list_images(c, &n.id).unwrap().iter().map(|i| i.id.clone()).collect::<Vec<_>>(), [a.id.clone(), b.id.clone()]);
    images::set_cover(c, &b.id).unwrap();
    assert_eq!(images::list_images(c, &n.id).unwrap()[0].id, b.id, "设为封面后排在最前");
    assert_eq!(images::covers(c, std::slice::from_ref(&n.id)).unwrap(), vec![(n.id.clone(), b.id.clone())]);
    images::set_caption(c, &a.id, "新的说明").unwrap();
    assert_eq!(images::get_image(c, &a.id).unwrap().caption, "新的说明");
    let url = images::data_url(c, &a.id).unwrap();
    assert!(url.starts_with("data:image/png;base64,iVBOR"));
    images::delete_image(c, &a.id).unwrap();
    assert_eq!(images::list_images(c, &n.id).unwrap().len(), 1);
    assert!(images::delete_image(c, &a.id).is_err());
}

#[test]
fn rejects_bad_input() {
    let mut e = env();
    let n = mk(&mut e.world, NewNode::new(NodeKind::Site, "旧码头"));
    let c = e.world.conn();
    assert!(images::add_image(c, &n.id, b"not an image at all", 1, 1, "").unwrap_err().to_string().contains("格式"));
    assert!(images::add_image(c, &n.id, &[], 1, 1, "").is_err());
    let mut big = png();
    big.resize(images::MAX_IMAGE_BYTES + 1, 0);
    assert!(images::add_image(c, &n.id, &big, 1, 1, "").unwrap_err().to_string().contains("太大"));
    assert!(images::add_image(c, "不存在的节点", &png(), 1, 1, "").is_err());
}

#[test]
fn images_survive_backup_and_follow_node_deletion() {
    let mut e = env();
    let n = mk(&mut e.world, NewNode::new(NodeKind::Item, "铜钥匙"));
    images::add_image(e.world.conn(), &n.id, &png(), 16, 16, "").unwrap();
    // 软删除的节点不能再加图片；永久删除时图片随之删除（外键级联）。
    e.world.write(&user(), "删", |w| w.delete_node(&n.id, None)).unwrap();
    assert!(images::add_image(e.world.conn(), &n.id, &png(), 16, 16, "").is_err());
    e.world.conn().execute("DELETE FROM nodes WHERE id = ?1", [&n.id]).unwrap();
    let left: i64 = e.world.conn().query_row("SELECT COUNT(*) FROM node_images", [], |r| r.get(0)).unwrap();
    assert_eq!(left, 0);
}

#[test]
fn bundle_round_trip_keeps_images_including_copy_mode() {
    let mut e = env();
    let n = mk(&mut e.world, NewNode::new(NodeKind::City, "雾港"));
    let img = images::add_image(e.world.conn(), &n.id, &png(), 16, 16, "雾港全景").unwrap();
    let path = e.dir.path().join("w.cgwb");
    let m = bundle::export_bundle(&e.world, &path, false).unwrap();
    assert_eq!(m.counts.get("images"), Some(&1));
    // 导入到另一个世界（保留 ID）。
    let mut other = env();
    let rep = bundle::import_bundle(&mut other.world, &path, ConflictStrategy::Skip, &user()).unwrap();
    assert_eq!(rep.images, 1);
    let got = images::list_images(other.world.conn(), &n.id).unwrap();
    assert_eq!((got[0].id.as_str(), got[0].caption.as_str()), (img.id.as_str(), "雾港全景"));
    assert_eq!(images::image_data(other.world.conn(), &img.id).unwrap().1, png());
    // 再导入一次（跳过）：不会重复。
    let rep = bundle::import_bundle(&mut other.world, &path, ConflictStrategy::Skip, &user()).unwrap();
    assert_eq!(rep.images, 0);
    // 复制模式：节点换了新 ID，图片跟着新节点走。
    let rep = bundle::import_bundle(&mut other.world, &path, ConflictStrategy::Copy, &user()).unwrap();
    assert_eq!(rep.images, 1);
    let copies = nodes::find_by_name(other.world.conn(), "雾港", None).unwrap();
    assert_eq!(copies.len(), 2);
    let copy = copies.iter().find(|c| c.id != n.id).unwrap();
    let ci = images::list_images(other.world.conn(), &copy.id).unwrap();
    assert_eq!(ci.len(), 1);
    assert_ne!(ci[0].id, img.id, "复制出的图片用新 ID");
    assert_eq!(images::image_data(other.world.conn(), &ci[0].id).unwrap().1, png());
}

#[test]
fn import_as_new_world_brings_images() {
    let mut e = env();
    let n = mk(&mut e.world, NewNode::new(NodeKind::Character, "神秘旅人"));
    images::add_image(e.world.conn(), &n.id, &png(), 16, 16, "斗篷").unwrap();
    let path = e.dir.path().join("w.cgwb");
    bundle::export_bundle(&e.world, &path, false).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let lib = Library::open(dir.path()).unwrap();
    let (_info, w, rep) = lib.import_world(&path, Some("副本")).unwrap();
    assert_eq!(rep.images, 1);
    assert_eq!(images::list_images(w.conn(), &n.id).unwrap()[0].caption, "斗篷");
}

/// 缩略图：保存与校验；封面缩略图跟着封面走；还没生成时为空；有图片的节点列表跳过已删除的节点。
#[test]
fn thumbs_covers_and_illustrated_nodes() {
    let mut e = env();
    let city = mk(&mut e.world, NewNode::new(NodeKind::City, "雾港"));
    let mei = mk(&mut e.world, NewNode::new(NodeKind::Character, "老板娘梅"));
    let bare = mk(&mut e.world, NewNode::new(NodeKind::Site, "旧码头"));
    let c = e.world.conn();
    let a = images::add_image(c, &city.id, &png(), 16, 16, "全景").unwrap();
    let b = images::add_image(c, &city.id, &jpeg_like(), 800, 600, "").unwrap();
    let m = images::add_image(c, &mei.id, &png(), 16, 16, "立绘").unwrap();

    // 还没有缩略图。
    let ids = vec![city.id.clone(), mei.id.clone(), bare.id.clone()];
    let cov = images::cover_thumbs(c, &ids).unwrap();
    assert_eq!(cov.len(), 2, "没有图片的节点不在结果里");
    assert_eq!((cov[0].image_id.as_str(), cov[0].count, cov[0].thumb.is_none()), (a.id.as_str(), 2, true));

    // 保存缩略图：只接受 JPEG/PNG/WebP，限制大小，图片要存在。
    images::set_thumb(c, &a.id, &jpeg_like()).unwrap();
    assert!(images::set_thumb(c, &b.id, b"GIF89a....").is_err(), "GIF 不能做缩略图");
    assert!(images::set_thumb(c, &b.id, b"not an image").is_err());
    assert!(images::set_thumb(c, &b.id, &[]).is_err());
    let mut big = vec![0xffu8, 0xd8, 0xff];
    big.resize(images::MAX_THUMB_BYTES + 1, 0);
    assert!(images::set_thumb(c, &b.id, &big).is_err(), "超过上限");
    assert!(images::set_thumb(c, "nope", &jpeg_like()).is_err());
    let cov = images::cover_thumbs(c, &ids).unwrap();
    assert!(cov[0].thumb.as_deref().unwrap().starts_with("data:image/jpeg;base64,"));
    assert_eq!(cov[1].caption, "立绘");

    // 换封面后，封面缩略图跟着换（新封面还没有缩略图）。
    images::set_cover(c, &b.id).unwrap();
    let cov = images::cover_thumbs(c, std::slice::from_ref(&city.id)).unwrap();
    assert_eq!((cov[0].image_id.as_str(), cov[0].thumb.is_none()), (b.id.as_str(), true));
    let all = images::node_thumbs(c, &city.id).unwrap();
    assert_eq!(all.iter().map(|t| t.image_id.as_str()).collect::<Vec<_>>(), [b.id.as_str(), a.id.as_str()]);
    assert!(all[1].thumb.is_some());

    // 缩略图不影响原图数据与导出元数据。
    assert_eq!(images::image_data(c, &a.id).unwrap().1, png());
    let _ = m;

    // 有图片的节点：按标题排序，软删除的节点不出现。
    let ill = images::illustrated_nodes(c).unwrap();
    assert_eq!(ill.iter().map(|n| (n.title.as_str(), n.count)).collect::<Vec<_>>(), [("老板娘梅", 1), ("雾港", 2)].iter().copied().sorted_by_title());
    e.world.write(&user(), "删", |w| w.delete_node(&mei.id, None)).unwrap();
    let ill = images::illustrated_nodes(e.world.conn()).unwrap();
    assert_eq!(ill.iter().map(|n| n.title.as_str()).collect::<Vec<_>>(), ["雾港"]);
}

trait SortedByTitle<'a> {
    fn sorted_by_title(self) -> Vec<(&'a str, i64)>;
}
impl<'a, I: Iterator<Item = (&'a str, i64)>> SortedByTitle<'a> for I {
    fn sorted_by_title(self) -> Vec<(&'a str, i64)> {
        let mut v: Vec<_> = self.collect();
        v.sort_by(|a, b| a.0.cmp(b.0));
        v
    }
}
