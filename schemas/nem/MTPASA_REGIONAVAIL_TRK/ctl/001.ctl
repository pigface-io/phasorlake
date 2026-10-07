-- ************************************************************
-- Title:	PUBLIC_ARCHIVE#MTPASA_REGIONAVAIL_TRK#FILE01#202408.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS MTPASA_REGIONAVAIL_TRK owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_ARCHIVE#MTPASA_REGIONAVAIL_TRK#FILE01#202408010000.CSV
APPEND INTO TABLE MTPASA_REGIONAVAIL_TRK
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
PUBLISH_DATETIME DATE "YYYY/MM/DD HH24:MI:SS",
STARTDATE DATE "YYYY/MM/DD HH24:MI:SS",
ENDDATE DATE "YYYY/MM/DD HH24:MI:SS",
LATEST_OFFER_DATETIME DATE "YYYY/MM/DD HH24:MI:SS")
-- End of Script
