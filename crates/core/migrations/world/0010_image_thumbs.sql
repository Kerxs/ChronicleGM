-- v10：节点图片的缩略图（长边 256 像素的 JPEG，由界面生成后保存）。
-- 旧图片的缩略图为空，界面第一次显示时补上。缩略图可以随时重新生成，不进入导入导出包。
ALTER TABLE node_images ADD COLUMN thumb BLOB;
