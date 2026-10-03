-- ************************************************************
-- Title:	PUBLIC_ARCHIVE#DISPATCHABLEUNIT#FILE01#202608.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS DISPATCHABLEUNIT owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_ARCHIVE#DISPATCHABLEUNIT#FILE01#202608010000.CSV
APPEND INTO TABLE DISPATCHABLEUNIT
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
DUID CHAR(10),
DUNAME CHAR(20),
UNITTYPE CHAR(20),
LASTCHANGED DATE "YYYY/MM/DD HH24:MI:SS")
-- End of Script
