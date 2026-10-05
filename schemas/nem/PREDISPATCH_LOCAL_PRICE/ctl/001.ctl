-- ************************************************************
-- Title:	PUBLIC_ARCHIVE#PREDISPATCH_LOCAL_PRICE#FILE01#202608.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS PREDISPATCH_LOCAL_PRICE owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_ARCHIVE#PREDISPATCH_LOCAL_PRICE#FILE01#202608010000.CSV
APPEND INTO TABLE PREDISPATCH_LOCAL_PRICE
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
PREDISPATCHSEQNO CHAR(20),
DATETIME DATE "YYYY/MM/DD HH24:MI:SS",
DUID CHAR(20),
PERIODID CHAR(20),
LOCAL_PRICE_ADJUSTMENT FLOAT EXTERNAL,
LOCALLY_CONSTRAINED FLOAT EXTERNAL,
LASTCHANGED DATE "YYYY/MM/DD HH24:MI:SS")
-- End of Script
