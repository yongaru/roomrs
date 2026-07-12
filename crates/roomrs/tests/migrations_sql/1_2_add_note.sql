-- v1 -> v2: note 컬럼 추가
ALTER TABLE "docs" ADD COLUMN "note" TEXT NOT NULL DEFAULT '';
