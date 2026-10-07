-- ************************************************************
-- Title:	PUBLIC_ARCHIVE#INTERMITTENT_GEN_LIMIT#FILE01#202408.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS INTERMITTENT_GEN_LIMIT owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_ARCHIVE#INTERMITTENT_GEN_LIMIT#FILE01#202408010000.CSV
APPEND INTO TABLE INTERMITTENT_GEN_LIMIT
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
TRADINGDATE DATE "YYYY/MM/DD HH24:MI:SS",
DUID CHAR(20),
OFFERDATETIME DATE "YYYY/MM/DD HH24:MI:SS",
PERIODID FLOAT EXTERNAL,
UPPERMWLIMIT FLOAT EXTERNAL)
-- End of Script
