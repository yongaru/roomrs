-- v2 -> v3: done 컬럼 추가
ALTER TABLE "docs" ADD COLUMN "done" INTEGER NOT NULL DEFAULT 0;
